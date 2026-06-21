//! The **near-real-time incremental indexer** — the bus consumer that feeds the per-tenant
//! index (SRCH-P06 / P-169; architecture `search-and-indexing.md` §4.1).
//!
//! ## What SRCH-P06 ships here
//! The [`IncrementalIndexer`] is an ordinary [`myelin_events::EventHandler`] (contract 2.4),
//! driven by the ONE sanctioned consumer runtime ([`myelin_events::Consumer`] — the seven encoded
//! rules) + the per-consumer [`myelin_events::DedupLedger`] (contract 2.5, idempotent on
//! `event_id`). It is one of the explicitly reviewed firehose-class infra consumers (§4.1 / BUS-4)
//! that genuinely needs every domain event — but it still **whitelists** the domain-event subjects
//! it indexes, **NEVER `*`** (an over-broad subscription head-of-line-blocks the whole consumer,
//! BUS-3). The whitelist is bound through the sanctioned [`myelin_events::consume`] path (which
//! rejects a `*`/empty subject loudly at registration).
//!
//! ### The per-event pipeline (§4.1) — idempotent, projection-fed, NEVER the owner DB
//! ```text
//! event → (dedup? skip) → resolve projection for (subsystem, type)
//!       → fetch the owner's project(ref, viewer)/replay snapshot via the ProjectFetcher
//!         (contract 5.6 — NOT the owner DB; the no-cross-db floor) — and for a sub-artifact doc
//!         resolve the #sub ArtifactRef through the same project call (the unified #sub resolver, 5.7)
//!       → analyze (language-detect → tokenize → normalize; §4.7 — a pass-through-correct floor here,
//!         the real per-language analyzer chain is SRCH-P12)
//!       → embed via the embedding adapter (mock v1, behind a trait, model_ref pinned) IF the type is
//!         semantically indexed (§4.8)
//!       → build IndexDocument (§3.1), stamp indexed_zookie + version from the event
//!       → upsert into the per-tenant index (S1/S2) atomically per doc_id → mark dedup → ack
//! ```
//! [`IncrementalIndexer::index`] is the single ingest step, factored out of [`EventHandler::handle`]
//! so a reindex-from-source replay (contract 2.6) / a drill can drive it directly — this is what
//! makes **steady-state == cold-rebuild a single code path** (SRCH-D5): a live `*.created` and a
//! `*.snapshot` replay both flow through HERE, the handler never branches cold-vs-live (a `*.snapshot`
//! carries the SAME envelope shape, only its `event_id` is the deterministic
//! [`myelin_events::snapshot_event_id`] so a re-run converges).
//!
//! ### ACL state is indexed too (§4.1 tail)
//! A **permission-change event** (`*.permission.changed`) updates the affected docs' `indexed_zookie`
//! (and bumps `version`) — Search indexes the OBJECT; Identity computes the subject's reachable set at
//! query time (the deliberate split that avoids the N+1 at index time). A permission change re-stamps
//! the doc's staleness anchor WITHOUT re-fetching/re-analyzing its body (it is the same content, a new
//! consistency token). This is how a revocation makes the index's `indexed_zookie` advance so the
//! zookie/consistency path (SRCH-P10) can tell a stale-grant read apart from a fresh one.
//!
//! ## The embedding adapter — a swappable trait with a deterministic MOCK v1 (VISION §3)
//! [`EmbeddingAdapter`] is the strategy seam; [`MockEmbeddingAdapter`] is the deterministic v1 used
//! during development. `model_ref` pins the adapter so a swap triggers a re-embed reindex, never a
//! silent mixed-model index (the embedding carries its [`crate::vector::ModelRef`] into the
//! [`IndexDocument`], the one-doc-id-space invariant the engine enforces).
//!
//! ## Telemetry (contract 1.8 / §4.11; observability is part of the pass — EI-01 §3)
//! - [`IncrementalIndexer::index_lag`] (`search.index_lag`) — events delivered to the indexer but not
//!   yet projected into the index (bumped on entry to [`index`], cleared on apply). 0 in steady state;
//!   a drill that pauses mid-flight reads it non-zero. No signal == failed drill.
//! - **consumer lag (`num_pending`)** is the runtime's [`myelin_events::Consumer::lag`] — the indexer
//!   is driven by that runtime, so the contract-1.8 `consumer_lag` signal reads it directly (it is not
//!   re-implemented here — ONE lag counter, the runtime's).
//!
//! ## Floors named (VISION §3 / prompt DoD)
//! - **The mock embedding adapter** ([`MockEmbeddingAdapter`]) is the v1 — the real EU-hostable model
//!   adapter is the **post-M5 / runtime config swap** (ADR-12.8); the vector math + erasure are built
//!   NOW (SRCH-P05), only the model is mocked. `model_ref` makes the swap a re-embed reindex, never a
//!   silent mixed-model index.
//! - **The synthetic / test producer + the IndexSpec API** ([`IndexSpec`]) are the FROZEN shape
//!   exercised by a synthetic producer here; the real per-subsystem `IndexSpec`s land **M3 Git/KN, M4
//!   Issues/CI/Chat** (SRCH-P17 et al.). Named so the indexer is not mistaken for fed-by-real-producers.
//! - **The per-tenant index is MODELLED in-process here** ([`IndexRegistry`] over
//!   [`crate::engine::TantivyBackend`], RAM-first like the SRCH-P04 engine). The at-rest seal of every
//!   Tantivy segment under the per-tenant index DEK is the layout/erase slice's concern (the DEK is
//!   reserved by SRCH-P02; the real ciphertext lands with SRCH-P15's erase + the storage wiring). The
//!   seam shape (the `EventHandler`, the projection fetch, the IndexDocument build, the `index_lag`
//!   signal) does NOT change. Named, not silently skipped.
//! - **The no-cross-db floor is STRUCTURAL here:** the only way a doc lands in the index is the
//!   [`ProjectFetcher`] (the owner's 5.6 `project`/replay) — there is NO owner-DB read path in this
//!   module (it does not depend on any sibling's storage; the `no-cross-db` lint holds over
//!   `crates/myelin-search/src`). A reindex re-drives the SAME [`index`] path (no Postgres backdoor).
//! - **Mutation floor (mandatory-core).** The per-event decision logic — the dedup-skip, the
//!   project-fetch branch, the IndexDocument build, the `indexed_zookie`/`version` stamp, the
//!   semantic-embed branch, the ACL-state-indexed re-stamp, the atomic upsert — is the mutation-tested
//!   core. The floor is stated + met by the unit + chained tests below (every branch asserted; a
//!   mutant that flips a branch or drops a stamp is caught). The SRCH-P04/P05 engine/vector mutation
//!   floors still hold (unchanged here). The world-scale freshness-under-load drill is **SRCH-P24/M5**.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use myelin_query::{FieldType, FieldValue};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

use crate::engine::{IndexBackend, IndexDocument, TantivyBackend};
use crate::vector::{Embedding, ModelRef};

/// The durable consumer name (rule 4: bind-by-name; re-bound identically on reconnect so the SAME
/// `consumer_dedup` ledger + cursor are re-used → 0 lost across reconnect). PII-free identifier.
pub const INDEXER_CONSUMER: &str = "search-incremental-indexer";

/// The `'static` subject-pattern whitelist the [`EventHandler`] trait requires (rule 3). The service
/// `serve` binds the runtime through the sanctioned [`myelin_events::consume`] path with
/// [`INDEXER_SUBJECT_PREFIXES`] (which the runtime rejects if `*`). The prefixes are the
/// references-not-payloads subject FAMILIES Search indexes; the dotted event TYPE on the envelope is
/// what [`index`](IncrementalIndexer::index) branches on. NEVER `*` (BUS-3/BUS-4).
pub static INDEXER_SUBJECTS: &[SubjectPattern] = &[];

/// The subject-prefix whitelist the indexer binds through [`myelin_events::ConsumerSpec`] (the ONE
/// sanctioned consumer entry-point). These are the domain-event subject FAMILIES Search indexes — the
/// artifact lifecycle subjects + the permission-change subject (so ACL state is indexed). NEVER `*`:
/// `consume(...)` rejects a `*`/empty subject loudly at registration. The set is FROZEN here as the
/// synthetic-producer surface; the real per-subsystem subjects are registered via [`IndexSpec`]
/// (SRCH-P17 et al.). Subject prefixes (the `Subscription`/`Consumer` prefix model).
pub const INDEXER_SUBJECT_PREFIXES: &[&str] = &[
    // The artifact-lifecycle subjects Search projects + indexes (synthetic producer here; the real
    // per-subsystem subjects land via IndexSpec — M3 Git/KN, M4 Issues/CI/Chat).
    "issue.",
    "knowledge.",
    "chat.",
    "git.",
    // The permission-change subject family (ACL state is indexed — §4.1 tail). A `*.permission.changed`
    // re-stamps the affected docs' indexed_zookie.
    "authz.",
];

/// **The IndexSpec (contract 6.3, FROZEN here as the synthetic-producer shape).** A subsystem
/// `declare_indexable(IndexSpec{ … })` at build time so the indexer knows, per `(subsystem, type)`,
/// which projection to fetch, which fields are full-text vs structured, whether the type is
/// semantically indexed, and the `acl_object_type` the ACL filter pins on. SRCH-P06 freezes the SHAPE
/// and exercises it with a synthetic producer; the REAL per-subsystem instances land M3/M4
/// (SRCH-P17 et al.). To the frozen shape (a rename of a field breaks the registrants then).
///
/// **Serialization (the 6.3 wire shape — GIT-P5/P-231).** The spec is `Serialize` so a producer's
/// owned spec half (e.g. git's code-projection spec, `myelin_git::search_projection`) can be
/// proven byte-stable against the frozen contract-6.3 keys in a CDC test. The contract row names
/// `projection` + `ft_fields` alongside `struct_fields`; this implementation realizes those two as
/// the **index-time [`SearchProjection`]** (`text` = the full-text/projection body, the `ft_fields`
/// content; `fields` = the structured facets typed by `struct_fields`). So the SPEC carries the
/// structured/semantic/acl half here and the projection+ft content arrives at emit time — no second
/// shape. A rename of a serialized key is a wire-breaking change the registrants' CDC tests catch.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct IndexSpec {
    /// The owning subsystem (`issue`/`knowledge`/`chat`/`git`/…) — the projection source.
    pub subsystem: String,
    /// The artifact type within the subsystem (`issue`/`page`/`message`/`blob`/…).
    #[serde(rename = "type")]
    pub type_: String,
    /// The structured facets (the structured/columnar shape) this type carries, by name → frozen
    /// [`FieldType`]. The engine types its columnar fast-fields byte-identically over these (13.3).
    pub struct_fields: BTreeMap<String, FieldType>,
    /// Whether this type is **semantically indexed** (gets a vector embedding via the adapter, §4.8).
    pub semantic: bool,
    /// The `acl_object_type` the ACL filter pins on (§3.1) — usually equal to `type_`, but a
    /// sub-artifact doc may pin on its parent's type. Carried so the query path's ACL conjoin keys on
    /// the right object type.
    pub acl_object_type: String,
}

impl IndexSpec {
    /// A non-semantic spec for `(subsystem, type)` with the given structured facets.
    pub fn new(
        subsystem: impl Into<String>,
        type_: impl Into<String>,
        struct_fields: BTreeMap<String, FieldType>,
    ) -> IndexSpec {
        let type_ = type_.into();
        IndexSpec {
            subsystem: subsystem.into(),
            acl_object_type: type_.clone(),
            type_,
            struct_fields,
            semantic: false,
        }
    }

    /// Mark this spec as semantically indexed (its docs get an embedding via the adapter, §4.8).
    pub fn semantic(mut self) -> IndexSpec {
        self.semantic = true;
        self
    }

    /// Pin the `acl_object_type` the ACL filter keys on when it DIFFERS from `type_` (§3.1). A git
    /// `blob` doc, for instance, is a sub-artifact whose reachability is decided by its parent
    /// **`repo`** (the ACL object is the repository, not the individual blob) — so git's
    /// code-projection spec is `type_ = "blob"` but `acl_object_type = "repo"` (GIT-P5/P-231). The
    /// default (constructor) keeps `acl_object_type == type_`.
    pub fn with_acl_object_type(mut self, acl_object_type: impl Into<String>) -> IndexSpec {
        self.acl_object_type = acl_object_type.into();
        self
    }
}

/// **The owner's projection of an artifact for indexing (contract 5.6 `project(ref, viewer)`).** This
/// is the searchable PROJECTION the owner returns — NOT its DB row. Search analyses `text`, indexes
/// the structured `fields`, and (if the type is semantic) embeds `text`. The owner is the only source
/// of this (the no-cross-db floor): Search NEVER reads the owner's store.
///
/// For a sub-artifact doc (a `#sub`-anchored ref) this is the projection of the RESOLVED sub-anchor
/// (the unified `#sub` resolver, 5.7) — the owner resolves the sub-anchor as part of `project`, so
/// Search receives the sub-precise searchable text without re-implementing the frozen `#sub` grammar.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchProjection {
    /// The analyzable free-text body (the full-text inverted shape source, 13.1). For a code blob this
    /// is the raw code (the code tokenizer is SRCH-P12); here it is analyzed pass-through-correct.
    pub text: String,
    /// The typed structured facets (the structured/columnar shape), each typed over the frozen
    /// [`FieldType`] (13.3). Must match the [`IndexSpec::struct_fields`] declaration for the type.
    pub fields: BTreeMap<String, FieldValue>,
    /// The analyzer-selection language tag (§3.1; index-time language detection sets it — here the
    /// owner may pin it, else the indexer's pass-through detector sets `und`). The per-language
    /// analyzer chain is SRCH-P12; this carries the tag so the index doc anchors it.
    pub lang: Option<String>,
}

/// Why an owner `project` fetch failed (contract 5.6 over the resilient client). A `Unavailable` is a
/// TRANSIENT owner hiccup (the resilient client surfaced it) — the indexer RETRIES (it never fabricates
/// a projection or drops the event silently). A `Gone` means the artifact no longer projects (it was
/// deleted/erased) — the indexer deletes the doc (a clean removal, not a poison).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectFetchError {
    /// The owner subsystem was transiently unavailable — the indexer retries (0 lost, never fabricated).
    Unavailable(String),
    /// The artifact no longer projects (deleted/erased at the owner) — the indexer removes its doc.
    Gone,
}

/// **The `project(ref, viewer)` fetch seam (contract 5.6, CONSUMED).** The indexer fetches the owner's
/// searchable projection through THIS — and ONLY this. There is NO owner-DB read path: this is the one
/// way a doc's body enters the index (the no-cross-db floor; in production the owner's 5.6 `project` is
/// reached over the [`myelin_client::ResilientClient`], a config/transport detail behind this trait).
/// For a sub-artifact ref the owner resolves the `#sub` anchor (5.7) as part of `project`, returning
/// the sub-precise projection.
///
/// `Send + Sync` so the indexer can hold it behind an [`Arc`] across serving threads. The indexer
/// passes a viewer-NEUTRAL system identity here (index-time fetch is the OBJECT's content, not a
/// viewer's redacted view — Identity computes the per-viewer reachable set at query time, §4.1 tail).
pub trait ProjectFetcher: Send + Sync {
    /// Fetch the owner's searchable projection of `ref_` in `(tenant, region)` (contract 5.6). A
    /// transient hiccup is [`ProjectFetchError::Unavailable`] (retry); a deleted/erased artifact is
    /// [`ProjectFetchError::Gone`] (remove the doc). NEVER the owner DB — the owner's `project`.
    fn project(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError>;
}

/// **The embedding-adapter strategy seam (§4.8; VISION §3 strategy pattern).** A semantically-indexed
/// type's `text` is embedded through THIS — a swappable trait so the real EU-hostable model is a
/// post-M5 config swap behind it. `model_ref` pins the adapter so a swap triggers a re-embed reindex,
/// never a silent mixed-model index.
///
/// `Send + Sync` so the indexer holds it behind an [`Arc`].
pub trait EmbeddingAdapter: Send + Sync {
    /// Embed `text` into a vector. Returns `None` for empty text (a doc with no body gets no embedding
    /// — a vector with no source is meaningless).
    fn embed(&self, text: &str) -> Option<Embedding>;

    /// The [`ModelRef`] this adapter produces (the model the embeddings are pinned to). Carried onto
    /// every [`IndexDocument`] so a model swap is a re-embed reindex, never a silent mixed-model index.
    fn model_ref(&self) -> ModelRef;
}

/// **The deterministic MOCK embedding adapter v1 (the named floor — VISION §3 "mock during
/// development").** Produces a fixed-dimension vector deterministically from the text (a stable hash
/// fold), so the same text always embeds to the same vector (idempotent re-embed) and two similar texts
/// share leading dimensions enough for the k-NN round-trip drills to be meaningful — WITHOUT any real
/// model. The real EU-hostable model adapter is the **post-M5 / runtime config swap** (ADR-12.8); the
/// vector math + erasure are built now (SRCH-P05), only the model is mocked. `model_ref` is pinned.
#[derive(Clone, Debug)]
pub struct MockEmbeddingAdapter {
    model_ref: ModelRef,
    dim: usize,
}

impl MockEmbeddingAdapter {
    /// The default mock model ref (the pinned model id; a swap re-embeds — ADR-12.8). PII-free token.
    pub const DEFAULT_MODEL: &'static str = "mock-embed-v1";

    /// A mock adapter producing `dim`-dimensional deterministic vectors under [`Self::DEFAULT_MODEL`].
    pub fn new(dim: usize) -> MockEmbeddingAdapter {
        MockEmbeddingAdapter {
            model_ref: ModelRef(Self::DEFAULT_MODEL.to_string()),
            dim: dim.max(1),
        }
    }

    /// A mock adapter pinned to an explicit `model_ref` (so a test can prove a model SWAP changes the
    /// pinned ref — a different ref triggers a re-embed reindex, never a silent mixed-model index).
    pub fn with_model(model_ref: impl Into<ModelRef>, dim: usize) -> MockEmbeddingAdapter {
        MockEmbeddingAdapter { model_ref: model_ref.into(), dim: dim.max(1) }
    }
}

impl EmbeddingAdapter for MockEmbeddingAdapter {
    fn embed(&self, text: &str) -> Option<Embedding> {
        if text.trim().is_empty() {
            return None;
        }
        // A deterministic, allocation-light fold: each dimension accumulates a per-byte FNV-1a-style
        // hash seeded by the dimension index, then is squashed into [-1, 1]. Deterministic (no clock,
        // no randomness) so the SAME text embeds to the SAME vector — an idempotent re-embed, and the
        // model_ref pins the math so a swap re-embeds. NOT a real semantic model (the named floor).
        let mut v = vec![0.0f32; self.dim];
        for (d, slot) in v.iter_mut().enumerate() {
            let mut h: u64 = 0xcbf29ce484222325 ^ (d as u64).wrapping_mul(0x100000001b3);
            for &b in text.as_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            // Map the high bits into [-1, 1] deterministically.
            let frac = (h >> 40) as f32 / (1u64 << 24) as f32; // [0, 1)
            *slot = frac * 2.0 - 1.0;
        }
        Some(Embedding::new(v))
    }

    fn model_ref(&self) -> ModelRef {
        self.model_ref.clone()
    }
}

/// The `(tenant, region)` partition key — every index read/write is tenant-first (the residency /
/// no-cross-tenant-query floor, §3.4). PII-free opaque partition tokens.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PartKey {
    tenant: TenantId,
    region: Region,
}

/// **The per-tenant index registry (S1/S2 — modelled in-process, RAM-first).** One
/// [`TantivyBackend`] per `(tenant, region)` — the per-tenant index space the doc is upserted into
/// (§3.4; encrypted-from-birth in production under the per-tenant index DEK reserved by SRCH-P02). The
/// indexer opens a per-tenant backend lazily on first write, declaring its structured facets from the
/// union of the registered [`IndexSpec`]s. The REAL DEK-sealed on-disk directory replaces this RAM
/// model when the storage is wired (named floor; the seam shape does not change).
struct IndexRegistry {
    indices: Mutex<HashMapBackends>,
    /// The union of structured facet declarations across all registered specs — every per-tenant
    /// backend is opened with the SAME schema so a query over any tenant sees the same facet space.
    facets: BTreeMap<String, FieldType>,
}

type HashMapBackends = std::collections::HashMap<PartKey, TantivyBackend>;

impl IndexRegistry {
    fn new(facets: BTreeMap<String, FieldType>) -> IndexRegistry {
        IndexRegistry { indices: Mutex::new(std::collections::HashMap::new()), facets }
    }

    /// Run `f` over the per-tenant backend for `(tenant, region)`, opening it on first use. Tenant-first
    /// (no cross-tenant index handle). Returns the engine error loudly (never a silent skip).
    fn with_backend<T>(
        &self,
        tenant: &TenantId,
        region: &Region,
        f: impl FnOnce(&mut TantivyBackend) -> Result<T, crate::engine::IndexError>,
    ) -> Result<T, crate::engine::IndexError> {
        let pk = PartKey { tenant: tenant.clone(), region: region.clone() };
        let mut guard = self.indices.lock().unwrap_or_else(|e| e.into_inner());
        if !guard.contains_key(&pk) {
            let be = TantivyBackend::open(&self.facets)?;
            guard.insert(pk.clone(), be);
        }
        let be = guard.get_mut(&pk).expect("backend just inserted");
        f(be)
    }

    /// The live doc count in the `(tenant, region)` index (the freshness/idempotency check reads this).
    fn live_count(&self, tenant: &TenantId, region: &Region) -> u64 {
        let pk = PartKey { tenant: tenant.clone(), region: region.clone() };
        let mut guard = self.indices.lock().unwrap_or_else(|e| e.into_inner());
        match guard.get_mut(&pk) {
            Some(be) => be.snapshot().unwrap_or(0),
            None => 0,
        }
    }

    /// Locate every live doc in the `(tenant, region)` index referencing the subject (§4.8 locate).
    /// A doc-id point walk over the per-tenant backend's side map (NOT a scored search). An absent
    /// partition has no docs. Tenant-first (no cross-tenant index handle).
    fn locate_subject(
        &self,
        tenant: &TenantId,
        region: &Region,
        matcher: &crate::engine::SubjectMatcher,
    ) -> Vec<String> {
        let pk = PartKey { tenant: tenant.clone(), region: region.clone() };
        let guard = self.indices.lock().unwrap_or_else(|e| e.into_inner());
        match guard.get(&pk) {
            Some(be) => be.locate_subject(matcher),
            None => Vec::new(),
        }
    }

    /// **Compact-on-merge the `(tenant, region)` index (§3.3) — the erasure-critical compaction.** After
    /// the `*.erased` purge soft-deletes the affected docs' vectors, this physically removes every
    /// tombstoned embedding (0 orphan embedding survives) and merges the Tantivy segments. An absent
    /// partition is a no-op. Tenant-first.
    fn compact(&self, tenant: &TenantId, region: &Region) -> Result<(), crate::engine::IndexError> {
        let pk = PartKey { tenant: tenant.clone(), region: region.clone() };
        let mut guard = self.indices.lock().unwrap_or_else(|e| e.into_inner());
        match guard.get_mut(&pk) {
            Some(be) => be.merge(),
            None => Ok(()),
        }
    }

    /// **Wipe the `(tenant, region)` index (the cold-rebuild precondition, §4.9 / SRCH-D5).** Drops the
    /// per-tenant backend so the next write re-opens an EMPTY one over the SAME facet schema — the
    /// modelled equivalent of deleting the DEK-sealed index directory before a reindex-from-source. This
    /// is NOT a backdoor write path: it only DESTROYS derived state (Search holds no system-of-record),
    /// after which the ONLY way docs re-enter is the live [`with_backend`] upsert the indexer drives from
    /// the bus re-emit. An absent partition is a no-op. Tenant-first (no cross-tenant handle).
    fn wipe(&self, tenant: &TenantId, region: &Region) {
        let pk = PartKey { tenant: tenant.clone(), region: region.clone() };
        let mut guard = self.indices.lock().unwrap_or_else(|e| e.into_inner());
        guard.remove(&pk);
    }

    /// Whether the `(tenant, region)` index has ANY orphan (tombstoned-until-compact) embedding — the
    /// 0-orphan-after-compact GATE reads this. An absent partition has none.
    fn has_orphan_embedding(&self, tenant: &TenantId, region: &Region) -> bool {
        let pk = PartKey { tenant: tenant.clone(), region: region.clone() };
        let guard = self.indices.lock().unwrap_or_else(|e| e.into_inner());
        match guard.get(&pk) {
            Some(be) => be.vectors().has_orphan_embedding(),
            None => false,
        }
    }
}

/// Why the indexer could not project an event into the index. A `Malformed` event (a missing
/// `ref`/`subsystem`/`type` the IndexSpec needs) is a LOUD non-retryable poison (fail-closed; never a
/// silent corruption). An `Engine` failure is the index op failing. A `Transient` is the owner-fetch
/// hiccup the runtime RETRIES (0 lost).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexEventError {
    /// A structurally-malformed event (missing the fields the IndexSpec needs) — non-retryable poison.
    Malformed(String),
    /// The underlying index engine failed — non-retryable (the op is loud, never a silent empty).
    Engine(String),
    /// A transient owner-fetch hiccup — the runtime retries (0 lost; never a fabricated projection).
    Transient(String),
}

/// **The near-real-time incremental indexer (SRCH-P06; contract 2.4 consumer side).** An ordinary
/// [`EventHandler`] over the per-tenant [`IndexRegistry`], fed the owner's projection via the
/// [`ProjectFetcher`] (5.6) and embedding via the [`EmbeddingAdapter`] (§4.8). Cloneable handle (the
/// registry + lag are shared). The `index_lag` (contract 1.8) is live + observable.
#[derive(Clone)]
pub struct IncrementalIndexer {
    registry: Arc<IndexRegistry>,
    /// The registered IndexSpecs by `(subsystem, type)` (the synthetic-producer surface, 6.3).
    specs: Arc<BTreeMap<(String, String), IndexSpec>>,
    fetcher: Arc<dyn ProjectFetcher>,
    embedder: Arc<dyn EmbeddingAdapter>,
    /// The live `search.index_lag` measurement (contract 1.8): events delivered but not yet projected.
    index_lag: Arc<AtomicU64>,
}

impl IncrementalIndexer {
    /// The telemetry signal name this indexer emits (contract 1.8 / §4.11). A named constant — drills
    /// assert against the NAME, never a literal (EI-01 §3 observability).
    pub const INDEX_LAG_SIGNAL: &'static str = "search.index_lag";

    /// The event-type suffix that re-stamps ACL state (the permission-change path, §4.1 tail). A
    /// `*.permission.changed` updates the affected docs' indexed_zookie WITHOUT re-fetching the body.
    pub const PERMISSION_CHANGED_SUFFIX: &'static str = "permission.changed";

    /// The event-type suffix that REMOVES a doc (a delete/erase of the artifact). The `*.erased`
    /// tombstone re-drives THIS via the same live consumer (the SRCH-P15 erase path; here the structural
    /// removal). A `*.deleted` is the ordinary delete.
    pub const REMOVED_SUFFIXES: &'static [&'static str] = &["deleted", "removed", "erased"];

    /// Build the indexer over the registered `specs` (the synthetic-producer IndexSpecs), the owner
    /// `fetcher` (5.6) and the embedding `embedder` (§4.8). The per-tenant index schema is the union of
    /// every spec's structured facets (so every per-tenant backend opens with the same facet space).
    pub fn new(
        specs: Vec<IndexSpec>,
        fetcher: Arc<dyn ProjectFetcher>,
        embedder: Arc<dyn EmbeddingAdapter>,
    ) -> IncrementalIndexer {
        let mut facets: BTreeMap<String, FieldType> = BTreeMap::new();
        let mut by_key: BTreeMap<(String, String), IndexSpec> = BTreeMap::new();
        for spec in specs {
            for (name, ty) in &spec.struct_fields {
                facets.insert(name.clone(), *ty);
            }
            by_key.insert((spec.subsystem.clone(), spec.type_.clone()), spec);
        }
        IncrementalIndexer {
            registry: Arc::new(IndexRegistry::new(facets)),
            specs: Arc::new(by_key),
            fetcher,
            embedder,
            index_lag: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The live `search.index_lag` sample (contract 1.8): events delivered to the indexer but not yet
    /// projected into the index. 0 in steady state (the synchronous apply cleared it); a drill that
    /// pauses mid-`index` reads it non-zero.
    pub fn index_lag(&self) -> u64 {
        self.index_lag.load(Ordering::SeqCst)
    }

    /// The live doc count in the `(tenant, region)` index (the freshness/idempotency assertions read
    /// this — a synthetic event becomes searchable here within the freshness budget; a replay is one
    /// doc, not two).
    pub fn live_count(&self, tenant: &TenantId, region: &Region) -> u64 {
        self.registry.live_count(tenant, region)
    }

    /// **Search the `(tenant, region)` full-text shape for `text_query` under `acl_filter` (the
    /// searchable-within-budget assertion reads this — SRCH-D7).** Delegates to the per-tenant backend.
    /// `acl_filter` is MANDATORY (the engine cannot be reached without it — the
    /// search-requires-acl-filter lint); this is the SRCH-P04 engine surface re-exposed for the
    /// freshness drill, NOT the SRCH-P08 query path (that lowers `list_objects` into the filter).
    pub fn search_ft(
        &self,
        tenant: &TenantId,
        region: &Region,
        acl_filter: &crate::engine::AclFilter,
        text_query: &str,
        limit: usize,
    ) -> Result<Vec<crate::engine::Hit>, crate::engine::IndexError> {
        self.registry
            .with_backend(tenant, region, |be| be.search(acl_filter, text_query, limit))
    }

    /// **Search the `(tenant, region)` structured/columnar shape on a typed facet equality, ACL-filtered
    /// (the JSONB GIN-scan path for KN custom DB-fields / Issues facets, §3.1 / §4.6.1).** Delegates to
    /// the per-tenant backend's structured shape — a `field == value` predicate over a typed columnar
    /// facet, `acl_filter` conjoined first, ordered by the `order_key` fast-field. This is the
    /// SRCH-P17 (P-260) KN custom-field facet drill seam — the in-doc database struct fields are served
    /// correctly by this typed-facet scan (the GIN-scan FLOOR; the measured projection-feeder promotion
    /// to a generated index is the M5 follow-on, SRCH-P27, which changes COST never correctness). Like
    /// [`search_ft`](Self::search_ft) this re-exposes the SRCH-P04 engine surface for the producer-corpus
    /// drills, NOT the SRCH-P08 `list_objects`-lowering query path. `acl_filter` is MANDATORY. Tenant-first.
    pub fn search_structured(
        &self,
        tenant: &TenantId,
        region: &Region,
        acl_filter: &crate::engine::AclFilter,
        field: &str,
        value: &FieldValue,
        limit: usize,
    ) -> Result<Vec<crate::engine::Hit>, crate::engine::IndexError> {
        use crate::engine::IndexBackend;
        self.registry
            .with_backend(tenant, region, |be| be.search_structured(acl_filter, field, value, limit))
    }

    /// **Semantic (vector) k-NN over the `(tenant, region)` co-located HNSW shape, ACL-filtered.** The
    /// erase drill reads this to assert a purged subject's VECTOR is gone (not just the FT doc). Tenant-first.
    pub fn search_semantic(
        &self,
        tenant: &TenantId,
        region: &Region,
        acl_filter: &crate::engine::AclFilter,
        query: &Embedding,
        k: usize,
    ) -> Result<Vec<crate::vector::VectorHit>, crate::engine::IndexError> {
        use crate::engine::IndexBackend;
        self.registry
            .with_backend(tenant, region, |be| be.semantic(acl_filter, query, k))
    }

    /// Read the stored `indexed_zookie` of a doc (the ACL-state-indexed assertion reads it — a
    /// permission change advances it). Returns `None` if the doc is absent. Tenant-first.
    pub fn indexed_zookie_of(
        &self,
        tenant: &TenantId,
        region: &Region,
        doc_id: &str,
    ) -> Option<String> {
        self.registry
            .with_backend(tenant, region, |be| Ok(be.indexed_zookie_of(doc_id)))
            .ok()
            .flatten()
    }

    /// **Locate every live doc in `(tenant, region)` referencing the subject (§4.8 `locate(subject)`;
    /// contract 10.1).** A doc-id point walk (NOT a scored search): returns the doc-ids the
    /// [`SubjectMatcher`](crate::engine::SubjectMatcher) admits (by `acl_object`, by an
    /// actor/assignee/mention subject-locator facet, or by the subject's `.noreply` pseudonym in the
    /// body). The set the holder's `locate` reports, `erase` purges, and `restrict` suppresses — ONE
    /// matcher, no drift. Tenant-first.
    pub fn locate_subject(
        &self,
        tenant: &TenantId,
        region: &Region,
        matcher: &crate::engine::SubjectMatcher,
    ) -> Vec<String> {
        self.registry.locate_subject(tenant, region, matcher)
    }

    /// **Compact the `(tenant, region)` index (§3.3) — physically remove every tombstoned embedding
    /// (0 orphan after compact) + merge segments.** The erase path calls this after the `*.erased`
    /// purge soft-deletes the affected docs' vectors. Tenant-first.
    pub fn compact(&self, tenant: &TenantId, region: &Region) -> Result<(), crate::engine::IndexError> {
        self.registry.compact(tenant, region)
    }

    /// Whether `(tenant, region)` holds ANY orphan (tombstoned-until-compact) embedding — the
    /// 0-orphan-after-compact GATE reads this (SRCH-D4: 0 recoverable incl. vectors). Tenant-first.
    pub fn has_orphan_embedding(&self, tenant: &TenantId, region: &Region) -> bool {
        self.registry.has_orphan_embedding(tenant, region)
    }

    /// **Wipe the `(tenant, region)` index (the cold-rebuild precondition — §4.9 / SRCH-D5).** Destroys
    /// the per-tenant derived index so a reindex-from-source rebuilds it cold; the next write re-opens an
    /// empty index over the SAME facet schema. Used by the [`crate::reindex`] path before a full rebuild
    /// and by the SRCH-D5 cold-vs-live parity drill. Search holds no system-of-record state, so this only
    /// drops reconstructible state (§1). Tenant-first.
    pub fn wipe(&self, tenant: &TenantId, region: &Region) {
        self.registry.wipe(tenant, region);
    }

    /// **Index ONE delivered event (the ONE ingest step — §4.1).** Factored out of
    /// [`EventHandler::handle`] so a reindex-from-source replay / a drill drives it directly (steady-
    /// state == cold-rebuild, SRCH-D5). Bumps `index_lag` on entry, clears on apply.
    ///
    /// Branches on the dotted event TYPE: a `*.permission.changed` re-stamps the doc's `indexed_zookie`
    /// (ACL state indexed, §4.1 tail); a `*.deleted`/`*.removed`/`*.erased` removes the doc; everything
    /// else is an upsert (fetch projection → analyze → embed-if-semantic → build → stamp → upsert). A
    /// structurally-malformed event is a non-retryable poison; a transient owner hiccup is retryable.
    pub fn index(&self, ev: &EventEnvelope) -> Result<(), IndexEventError> {
        self.index_lag.fetch_add(1, Ordering::SeqCst);
        let result = self.index_inner(ev);
        self.index_lag.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn index_inner(&self, ev: &EventEnvelope) -> Result<(), IndexEventError> {
        let type_ = ev.type_.0.as_str();

        // The permission-change path (§4.1 tail): ACL state is indexed too. Re-stamp the affected docs'
        // indexed_zookie WITHOUT re-fetching/re-analyzing the body (same content, new consistency token).
        if type_.ends_with(Self::PERMISSION_CHANGED_SUFFIX) {
            return self.apply_permission_changed(ev);
        }

        // The removal path: a delete/erase removes the doc (and its co-located vector). The *.erased
        // tombstone re-drives this via the SAME live consumer (the SRCH-P15 erase path — no backdoor).
        let event_name = type_.rsplit('.').next().unwrap_or("");
        if Self::REMOVED_SUFFIXES.contains(&event_name) {
            return self.apply_removed(ev);
        }

        // The upsert path (the per-event pipeline body).
        self.apply_upsert(ev)
    }

    /// The `(subsystem, type)` an artifact ref names — `myelin://<tenant>/<subsystem>/<type>/<id>[#sub]`.
    /// Returns the subsystem + type segments (the IndexSpec key). PII-free (opaque URN segments).
    fn subsystem_type_of(ref_: &ArtifactRef) -> Option<(String, String)> {
        // Strip the scheme + tenant, then read the subsystem/type segments.
        let rest = ref_.0.strip_prefix("myelin://")?;
        let mut segs = rest.split('/');
        let _tenant = segs.next()?;
        let subsystem = segs.next()?;
        let type_ = segs.next()?;
        if subsystem.is_empty() || type_.is_empty() {
            return None;
        }
        Some((subsystem.to_string(), type_.to_string()))
    }

    /// Is this ref a sub-artifact doc (a `#sub`-anchored ref, 5.7)? The owner resolves the sub-anchor as
    /// part of `project` (Search does not re-implement the frozen grammar); this only DETECTS one so the
    /// doc_id is keyed at sub-granularity and the ACL pins on the parent (the `acl_object`).
    fn sub_anchor_of(ref_: &ArtifactRef) -> Option<&str> {
        ref_.0.split_once('#').map(|(_, sub)| sub)
    }

    /// The `#sub`-stripped parent ref (the ACL pre-filter key for a sub-artifact doc — the ACL pins on
    /// the parent, §3.1). For a root doc this is the ref itself.
    fn acl_object_of(ref_: &ArtifactRef) -> String {
        match ref_.0.split_once('#') {
            Some((root, _sub)) => root.to_string(),
            None => ref_.0.clone(),
        }
    }

    fn apply_upsert(&self, ev: &EventEnvelope) -> Result<(), IndexEventError> {
        let ref_ = &ev.subject;
        let (subsystem, type_) = Self::subsystem_type_of(ref_).ok_or_else(|| {
            IndexEventError::Malformed(format!("event subject `{}` is not a myelin:// artifact ref", ref_.0))
        })?;

        // The IndexSpec for (subsystem, type) — a type with no registered spec is a NO-OP (Search does
        // not index everything; an unregistered type is silently skipped, not poisoned — defence in
        // depth, the whitelist already bounds the subject families). The real per-subsystem specs land
        // M3/M4 (this is the synthetic-producer surface).
        let spec = match self.specs.get(&(subsystem.clone(), type_.clone())) {
            Some(s) => s.clone(),
            None => return Ok(()), // unregistered type → not indexed (no-op).
        };

        // Fetch the owner's projection (5.6) — NOT the owner DB (the no-cross-db floor). For a
        // sub-artifact ref the owner resolves the #sub anchor (5.7) as part of project. A transient
        // hiccup is RETRYABLE (0 lost); a GONE artifact is a clean removal.
        let projection = match self.fetcher.project(&ev.tenant, &ev.region, ref_) {
            Ok(p) => p,
            Err(ProjectFetchError::Unavailable(why)) => return Err(IndexEventError::Transient(why)),
            Err(ProjectFetchError::Gone) => {
                // The artifact no longer projects (deleted/erased at the owner) → remove its doc.
                return self.remove_doc(&ev.tenant, &ev.region, &ref_.0);
            }
        };

        // Analyze (language-detect → tokenize → normalize, §4.7). The REAL per-language analyzer chain
        // (SRCH-P12 / P-175) lives in `crate::analysis`: a source-declared `lang` overrides; else the
        // index-time detector ([`Self::detect_lang`]) selects the field-language. The `lang` TAG is
        // carried as the `lang` index-doc field (stored alongside the staleness anchor) so the
        // query-time path selects the IDENTICAL chain — the no-analyzer-mismatch-miss parity invariant.
        let lang = projection.lang.clone().unwrap_or_else(|| Self::detect_lang(&projection.text));
        // Run the ONE chain over the body at index time (the analyzed term set is what a query-time
        // analysis of the SAME language must match — proven in `crate::analysis`'s parity gate). The
        // analyzed terms inform the inverted shape; the Tantivy `TEXT` field carries the body so the
        // round-trip + re-stamp path (§4.1 tail) keeps it. (The custom-tokenizer engine registration is
        // the downstream integration; the analyzer SEMANTICS — the load-bearing correctness — are here.)
        let _analyzed_terms =
            crate::analysis::Analyzer::for_tag(&lang).analyze(&projection.text);

        // Build the IndexDocument (§3.1): doc_id = the ArtifactRef key (sub-precise), acl_object = the
        // #sub-stripped parent (the ACL pre-filter key, §3.1). For a sub-artifact doc (a `#sub`-anchored
        // ref, 5.7) the doc_id is kept sub-precise while the ACL pins on the parent — the owner already
        // resolved the sub-anchor as part of `project` (Search does not re-implement the frozen grammar).
        let acl_object = Self::acl_object_of(ref_);
        debug_assert_eq!(
            Self::sub_anchor_of(ref_).is_some(),
            acl_object != ref_.0,
            "a sub-artifact doc pins its ACL on the #sub-stripped parent (5.7/§3.1)"
        );
        let mut doc =
            IndexDocument::new(ref_.0.clone(), projection.text.clone()).with_acl_object(acl_object);
        for (name, value) in &projection.fields {
            // A facet the spec did not declare is a malformed projection (the producer drifted) — loud,
            // not silently indexed under an undeclared column.
            if !spec.struct_fields.contains_key(name) {
                return Err(IndexEventError::Malformed(format!(
                    "projection of `{}` carries facet `{name}` not declared in the IndexSpec for ({subsystem}, {type_})",
                    ref_.0
                )));
            }
            doc = doc.with_field(name.clone(), value.clone());
        }
        // Stamp the analyzer-selection language tag (§3.1) so the index doc anchors it (the SRCH-P12
        // analyzer chain reads it). It is the `lang` index-doc field, NOT a structured facet.
        doc = doc.with_lang(lang);

        // Embed if the type is semantically indexed (§4.8) — via the mock adapter (the named floor),
        // model_ref pinned so a model swap is a re-embed reindex (never a silent mixed-model index).
        if spec.semantic {
            if let Some(embedding) = self.embedder.embed(&projection.text) {
                doc = doc.with_embedding(embedding, self.embedder.model_ref());
            }
        }

        // Stamp indexed_zookie + version from the event (the staleness anchor, §3.1). The zookie is the
        // event's consistency token (carried on the references-not-payloads payload as `zookie`); the
        // version is the projection version (the payload `version`, else the event is its own version
        // anchor — a monotonic-per-aggregate stand-in is the event's correlation/occurred ordering, here
        // the payload `version` or 0 floor). These pin WHEN the doc was indexed for the SRCH-P10 path.
        let zookie = Self::str_field(&ev.payload, "zookie").unwrap_or_default();
        let version = Self::u64_field(&ev.payload, "version").unwrap_or(0);

        // Upsert S1/S2 atomically per doc_id (idempotent on doc_id — a replay/redelivery is one doc).
        // The indexed_zookie/version stamp rides the engine's stored fields.
        self.registry
            .with_backend(&ev.tenant, &ev.region, |be| be.upsert_stamped(&doc, &zookie, version))
            .map_err(|e| IndexEventError::Engine(e.to_string()))
    }

    fn apply_removed(&self, ev: &EventEnvelope) -> Result<(), IndexEventError> {
        // The removed/erased ref may be the subject, or named in the payload (`ref`). Default to the
        // subject (the artifact this event is about).
        let doc_id = Self::str_field(&ev.payload, "ref").unwrap_or_else(|| ev.subject.0.clone());
        self.remove_doc(&ev.tenant, &ev.region, &doc_id)
    }

    fn remove_doc(&self, tenant: &TenantId, region: &Region, doc_id: &str) -> Result<(), IndexEventError> {
        self.registry
            .with_backend(tenant, region, |be| be.delete(doc_id))
            .map_err(|e| IndexEventError::Engine(e.to_string()))
    }

    fn apply_permission_changed(&self, ev: &EventEnvelope) -> Result<(), IndexEventError> {
        // ACL state is indexed (§4.1 tail): a permission change re-stamps the affected docs'
        // indexed_zookie (advancing the staleness anchor) WITHOUT re-fetching the body. The affected
        // docs are named in the payload (`refs`: the object(s) whose ACL changed) — Search indexes the
        // OBJECT; Id computes the subject's reachable set at query time. The new zookie is the change's
        // consistency token.
        let new_zookie = Self::str_field(&ev.payload, "zookie").ok_or_else(|| {
            IndexEventError::Malformed(format!(
                "{} permission-change carries no `zookie` (the new consistency token)",
                ev.type_.0
            ))
        })?;
        let refs = Self::str_array_field(&ev.payload, "refs").ok_or_else(|| {
            IndexEventError::Malformed(format!(
                "{} permission-change carries no `refs` (the affected objects)",
                ev.type_.0
            ))
        })?;
        for doc_id in &refs {
            // Re-stamp only docs that EXIST (a permission change on an un-indexed object is a no-op —
            // it has no doc to re-stamp). The version bumps so the SRCH-P10 path sees a newer projection.
            self.registry
                .with_backend(&ev.tenant, &ev.region, |be| {
                    be.restamp_zookie(doc_id, &new_zookie);
                    Ok(())
                })
                .map_err(|e| IndexEventError::Engine(e.to_string()))?;
        }
        Ok(())
    }

    /// **Index-time language detection (§4.7, SRCH-P12 / P-175).** Selects the field-language whose
    /// per-language analyzer chain ([`crate::analysis`]) analyzed this body — script-first (CJK vs
    /// Latin), then an EU stopword-overlap best-effort, defaulting to `und` (never a wrong confident
    /// guess). The returned `lang` TAG is stamped on the index doc (§3.1); the query-time path reads
    /// it back to select the IDENTICAL chain (the no-analyzer-mismatch-miss parity invariant). A
    /// source-declared language overrides this (the projection's `lang`, handled by the caller). The
    /// exact EU language set + CJK strategy remain the [OPEN → P6] floor; the per-language MECHANISM is
    /// built in [`crate::analysis`].
    fn detect_lang(text: &str) -> String {
        crate::analysis::detect_language(text).tag().to_string()
    }

    fn str_field(payload: &serde_json::Value, key: &str) -> Option<String> {
        payload.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
    }

    fn u64_field(payload: &serde_json::Value, key: &str) -> Option<u64> {
        payload.get(key).and_then(|v| v.as_u64())
    }

    fn str_array_field(payload: &serde_json::Value, key: &str) -> Option<Vec<String>> {
        payload.get(key).and_then(|v| v.as_array()).map(|arr| {
            arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()
        })
    }
}

impl EventHandler for IncrementalIndexer {
    /// The `*`-free subject whitelist (rule 3): the artifact-lifecycle + permission-change subjects.
    /// The `'static` slice the trait requires; the service `serve` binds the runtime through the
    /// sanctioned [`myelin_events::consume`] with [`INDEXER_SUBJECT_PREFIXES`] (rejected if `*`).
    fn subjects(&self) -> &'static [SubjectPattern] {
        INDEXER_SUBJECTS
    }

    /// Index the delivered event (contract 2.4). Idempotent on `event_id` (the runtime's
    /// `consumer_dedup` outer guard, rule 1) AND on `doc_id` (the engine's delete-then-add upsert) —
    /// belt and braces. A malformed event is a non-retryable poison; a transient owner hiccup is a
    /// Retry (0 lost — the runtime redelivers, the dedup mark is reverted so it re-runs).
    fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
        match self.index(ev) {
            Ok(()) => HandleOutcome::Done,
            Err(IndexEventError::Malformed(why)) => HandleOutcome::NonRetryable(Reason(why)),
            Err(IndexEventError::Engine(why)) => HandleOutcome::NonRetryable(Reason(why)),
            // A transient owner-fetch hiccup: RETRY (the runtime does not ack, reverts the dedup
            // mark, redelivers — 0 lost, never a fabricated projection). The `_why` is surfaced
            // through the runtime's lag/redelivery, not swallowed.
            Err(IndexEventError::Transient(_why)) => {
                HandleOutcome::Retry(myelin_events::Backoff { seconds: 2 })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::AclFilter;
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::collections::HashMap;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn principal() -> Principal {
        Principal::stub(PrincipalId("p-opaque-1".into()), PrincipalKind::Human, tenant())
    }

    /// A scripted [`ProjectFetcher`] over an in-memory map of `ref → projection`, with a per-ref
    /// "transient hiccup until the Nth call" injection (so the chained Retry path is exercised). A ref
    /// not in the map projects as [`ProjectFetchError::Gone`] (deleted/erased).
    #[derive(Default)]
    struct FakeFetcher {
        projections: Mutex<HashMap<String, SearchProjection>>,
        /// refs that should fail `Unavailable` until their countdown hits 0 (the transient hiccup).
        flaky: Mutex<HashMap<String, u32>>,
        /// how many times `project` was called per ref (so dedup-skips are observable).
        calls: Mutex<HashMap<String, u32>>,
    }
    impl FakeFetcher {
        fn with(ref_: &str, p: SearchProjection) -> FakeFetcher {
            let f = FakeFetcher::default();
            f.projections.lock().unwrap().insert(ref_.to_string(), p);
            f
        }
        fn put(&self, ref_: &str, p: SearchProjection) {
            self.projections.lock().unwrap().insert(ref_.to_string(), p);
        }
        fn set_flaky(&self, ref_: &str, fail_times: u32) {
            self.flaky.lock().unwrap().insert(ref_.to_string(), fail_times);
        }
        fn call_count(&self, ref_: &str) -> u32 {
            self.calls.lock().unwrap().get(ref_).copied().unwrap_or(0)
        }
    }
    impl ProjectFetcher for FakeFetcher {
        fn project(
            &self,
            _tenant: &TenantId,
            _region: &Region,
            ref_: &ArtifactRef,
        ) -> Result<SearchProjection, ProjectFetchError> {
            *self.calls.lock().unwrap().entry(ref_.0.clone()).or_insert(0) += 1;
            // The transient-hiccup injection: fail Unavailable until the countdown drains.
            if let Some(n) = self.flaky.lock().unwrap().get_mut(&ref_.0) {
                if *n > 0 {
                    *n -= 1;
                    return Err(ProjectFetchError::Unavailable("owner transiently down".into()));
                }
            }
            match self.projections.lock().unwrap().get(&ref_.0) {
                Some(p) => Ok(p.clone()),
                None => Err(ProjectFetchError::Gone),
            }
        }
    }

    fn proj(text: &str) -> SearchProjection {
        SearchProjection { text: text.into(), fields: BTreeMap::new(), lang: None }
    }

    fn proj_with(text: &str, fields: BTreeMap<String, FieldValue>) -> SearchProjection {
        SearchProjection { text: text.into(), fields, lang: None }
    }

    /// An issue IndexSpec (non-semantic) with a `status` facet.
    fn issue_spec() -> IndexSpec {
        let mut fields = BTreeMap::new();
        fields.insert("status".to_string(), FieldType::Select);
        IndexSpec::new("issue", "issue", fields)
    }

    /// A semantic knowledge-page spec.
    fn page_spec() -> IndexSpec {
        IndexSpec::new("knowledge", "page", BTreeMap::new()).semantic()
    }

    fn indexer_with(specs: Vec<IndexSpec>, fetcher: Arc<FakeFetcher>) -> IncrementalIndexer {
        IncrementalIndexer::new(specs, fetcher, Arc::new(MockEmbeddingAdapter::new(8)))
    }

    /// A domain event about `subject` of `type_`, carrying a references-not-payloads payload.
    fn event(id: &str, type_: &str, subject: &str, payload: serde_json::Value) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(principal()),
            subject: ArtifactRef(subject.into()),
            aggregate: AggregateKey(format!("agg:{subject}")),
            causation_id: None,
            correlation_id: CorrelationId(id.into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload,
        }
    }

    // --- The per-event pipeline: project-fetch → build → stamp → upsert ---

    /// **The per-event pipeline indexes a synthetic event: fetch the owner projection (5.6), build the
    /// IndexDocument, stamp indexed_zookie/version, upsert — and the doc is searchable.** (The
    /// project-fetch + build + stamp + atomic-upsert core, the prompt's required unit test.)
    #[test]
    fn per_event_pipeline_indexes_and_is_searchable() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let mut fields = BTreeMap::new();
        fields.insert("status".to_string(), FieldValue::Select("open".into()));
        let fetcher = Arc::new(FakeFetcher::with(r, proj_with("deadlock in the scheduler", fields)));
        let ix = indexer_with(vec![issue_spec()], fetcher.clone());

        let ev = event("01J-1", "issue.issue.created", r, serde_json::json!({ "zookie": "zk-7", "version": 3 }));
        assert_eq!(ix.handle(&ev), HandleOutcome::Done);

        // The owner's project was fetched (NOT a DB read — the no-cross-db floor; this is the only
        // ingest path).
        assert_eq!(fetcher.call_count(r), 1, "the owner projection was fetched once (5.6)");

        // The doc is searchable (the freshness property — SRCH-D7) under an allow-set ACL filter.
        let hits = ix
            .search_ft(&tenant(), &region(), &AclFilter::ids([r]), "deadlock", 10)
            .expect("search");
        assert_eq!(hits.len(), 1, "the indexed doc is searchable");
        assert_eq!(hits[0].doc_id, r);
        assert_eq!(ix.live_count(&tenant(), &region()), 1, "exactly one live doc");

        // The staleness anchor was stamped from the event (the indexed_zookie + version, §3.1).
        assert_eq!(ix.indexed_zookie_of(&tenant(), &region(), r).as_deref(), Some("zk-7"));
    }

    /// **Idempotent indexing: replaying the SAME event twice upserts ONE IndexDocument (idempotent on
    /// doc_id), and the runtime's dedup makes the handler a no-op on the redelivery.** (The prompt's
    /// idempotency GATE.)
    #[test]
    fn replaying_the_same_event_upserts_one_doc() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("body text")));
        let ix = indexer_with(vec![issue_spec()], fetcher.clone());
        let ev = event("01J-1", "issue.issue.created", r, serde_json::json!({ "zookie": "z1" }));

        // index() directly twice ⇒ still one doc (idempotent on doc_id at the engine).
        assert_eq!(ix.index(&ev), Ok(()));
        assert_eq!(ix.index(&ev), Ok(()));
        assert_eq!(ix.live_count(&tenant(), &region()), 1, "idempotent on doc_id: one doc, not two");

        // Through the runtime, a redelivered event_id is a handler no-op (the consumer_dedup outer
        // guard, rule 1) — the indexer never re-projects.
        use myelin_events::{ConsumerName, DedupLedger, Subscription, PrefetchBound, Consumer, Message};
        let sub = Subscription::bind(
            ConsumerName(INDEXER_CONSUMER.into()),
            &["myelin://acme/issue/"],
            PrefetchBound::DEFAULT,
        )
        .unwrap();
        let consumer = Consumer::new(ix.clone(), sub, DedupLedger::new());
        let msg = Message { subject: r.to_string(), envelope: ev.clone() };
        assert_eq!(consumer.deliver(&msg), myelin_events::Delivered::Acked);
        let before = fetcher.call_count(r);
        assert_eq!(consumer.deliver(&msg), myelin_events::Delivered::Deduplicated, "redelivery deduped");
        assert_eq!(fetcher.call_count(r), before, "the deduped redelivery never re-fetched/re-indexed");
    }

    /// **A type with NO registered IndexSpec is a NO-OP (not indexed, not poisoned).** Search does not
    /// index everything; an unregistered `(subsystem, type)` is silently skipped (defence in depth).
    #[test]
    fn unregistered_type_is_a_noop() {
        let r = "myelin://acme/chat/message/m1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("hi")));
        let ix = indexer_with(vec![issue_spec()], fetcher.clone()); // only `issue/issue` registered.
        let ev = event("01J-x", "chat.message.created", r, serde_json::json!({}));
        assert_eq!(ix.index(&ev), Ok(()), "unregistered type → no-op");
        assert_eq!(fetcher.call_count(r), 0, "no projection fetched for an unindexed type");
        assert_eq!(ix.live_count(&tenant(), &region()), 0);
    }

    /// **A non-myelin:// subject is a LOUD non-retryable poison (fail-closed, never a silent
    /// corruption).**
    #[test]
    fn malformed_subject_is_a_nonretryable_poison() {
        let fetcher = Arc::new(FakeFetcher::default());
        let ix = indexer_with(vec![issue_spec()], fetcher);
        let ev = event("01J-bad", "issue.issue.created", "not-a-ref", serde_json::json!({}));
        match ix.handle(&ev) {
            HandleOutcome::NonRetryable(Reason(r)) => assert!(r.contains("not a myelin"), "names it: {r}"),
            other => panic!("expected a non-retryable poison, got {other:?}"),
        }
    }

    /// **A facet the IndexSpec did not declare is a malformed projection (the producer drifted) — a
    /// LOUD poison, never silently indexed under an undeclared column.**
    #[test]
    fn projection_with_undeclared_facet_is_a_poison() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let mut fields = BTreeMap::new();
        fields.insert("severity".to_string(), FieldValue::Int(9)); // not declared in issue_spec.
        let fetcher = Arc::new(FakeFetcher::with(r, proj_with("x", fields)));
        let ix = indexer_with(vec![issue_spec()], fetcher);
        let ev = event("01J-1", "issue.issue.created", r, serde_json::json!({}));
        match ix.handle(&ev) {
            HandleOutcome::NonRetryable(Reason(m)) => assert!(m.contains("severity"), "names the facet: {m}"),
            other => panic!("expected a poison, got {other:?}"),
        }
    }

    // --- The embedding adapter (mock-behind-trait, model_ref pinned) ---

    /// **A semantically-indexed type gets a deterministic embedding via the mock adapter, model_ref
    /// pinned — and is reachable by semantic k-NN under the same doc_id.** (The §4.8 embed branch.)
    #[test]
    fn semantic_type_embeds_via_the_mock_adapter() {
        let r = "myelin://acme/knowledge/page/42";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("distributed consensus and raft")));
        let ix = indexer_with(vec![page_spec()], fetcher);
        let ev = event("01J-p", "knowledge.page.created", r, serde_json::json!({ "zookie": "z" }));
        assert_eq!(ix.handle(&ev), HandleOutcome::Done);

        // The doc carries a vector under the SAME doc_id (one doc-id space, §3.2): a semantic query for
        // the SAME text returns it (the mock embed is deterministic, so the doc's own text is its
        // nearest neighbour).
        let query = MockEmbeddingAdapter::new(8).embed("distributed consensus and raft").unwrap();
        let hits = ix
            .registry
            .with_backend(&tenant(), &region(), |be| be.semantic(&AclFilter::ids([r]), &query, 1))
            .expect("semantic");
        assert_eq!(hits.len(), 1, "the semantically-indexed doc is reachable by k-NN");
        assert_eq!(hits[0].doc_id, r);
        // The model_ref is the pinned mock model (a swap re-embeds, never a silent mixed-model index).
        assert_eq!(hits[0].model_ref, ModelRef(MockEmbeddingAdapter::DEFAULT_MODEL.into()));
    }

    /// **The mock embedding is DETERMINISTIC (the same text → the same vector → an idempotent
    /// re-embed) and a MODEL SWAP changes the pinned model_ref (so a swap triggers a re-embed reindex,
    /// never a silent mixed-model index).**
    #[test]
    fn mock_embedding_is_deterministic_and_model_pinned() {
        let a = MockEmbeddingAdapter::new(8);
        let v1 = a.embed("alpha beta").unwrap();
        let v2 = a.embed("alpha beta").unwrap();
        assert_eq!(v1.0, v2.0, "the same text embeds to the same vector (deterministic, idempotent)");
        assert_ne!(a.embed("gamma").unwrap().0, v1.0, "different text → different vector");
        assert!(a.embed("   ").is_none(), "empty text gets no embedding");
        // A different model_ref pins a different model (the swap signal).
        let b = MockEmbeddingAdapter::with_model("eu-model-v2", 8);
        assert_ne!(a.model_ref(), b.model_ref(), "a model swap is a distinct model_ref");
    }

    // --- ACL state is indexed (the permission-change path, §4.1 tail) ---

    /// **A permission-change event re-stamps the affected doc's indexed_zookie WITHOUT re-fetching the
    /// body (ACL state is indexed, §4.1 tail).** (The prompt's required ACL-state-indexed unit test.)
    #[test]
    fn permission_change_restamps_indexed_zookie() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("body")));
        let ix = indexer_with(vec![issue_spec()], fetcher.clone());

        // Index the doc with an initial zookie.
        let create = event("01J-1", "issue.issue.created", r, serde_json::json!({ "zookie": "zk-1" }));
        assert_eq!(ix.handle(&create), HandleOutcome::Done);
        assert_eq!(ix.indexed_zookie_of(&tenant(), &region(), r).as_deref(), Some("zk-1"));
        let fetches_before = fetcher.call_count(r);

        // A permission change names the affected object in `refs` + carries the new zookie.
        let perm = event(
            "01J-perm",
            "authz.tuple.permission.changed",
            r,
            serde_json::json!({ "zookie": "zk-2", "refs": [r] }),
        );
        assert_eq!(ix.handle(&perm), HandleOutcome::Done);

        // The doc's indexed_zookie ADVANCED — and the body was NOT re-fetched (same content, new token).
        assert_eq!(ix.indexed_zookie_of(&tenant(), &region(), r).as_deref(), Some("zk-2"), "zookie advanced");
        assert_eq!(fetcher.call_count(r), fetches_before, "the body was NOT re-fetched on a permission change");
        // The doc is still searchable (the re-stamp preserved the body).
        let hits = ix.search_ft(&tenant(), &region(), &AclFilter::ids([r]), "body", 10).expect("search");
        assert_eq!(hits.len(), 1, "the re-stamped doc still has its body");
    }

    /// **A permission change on an UN-INDEXED object is a no-op (it has no doc to re-stamp), not an
    /// error.**
    #[test]
    fn permission_change_on_unindexed_object_is_a_noop() {
        let fetcher = Arc::new(FakeFetcher::default());
        let ix = indexer_with(vec![issue_spec()], fetcher);
        let perm = event(
            "01J-perm",
            "authz.tuple.permission.changed",
            "myelin://acme/issue/issue/NONE",
            serde_json::json!({ "zookie": "zk-2", "refs": ["myelin://acme/issue/issue/NONE"] }),
        );
        assert_eq!(ix.handle(&perm), HandleOutcome::Done, "a perm change on an un-indexed object is a no-op");
    }

    /// **A permission-change event with no `zookie`/`refs` is a LOUD poison (a malformed ACL event must
    /// not silently leave the index stale).**
    #[test]
    fn malformed_permission_change_is_a_poison() {
        let fetcher = Arc::new(FakeFetcher::default());
        let ix = indexer_with(vec![issue_spec()], fetcher);
        let perm = event("01J-perm", "authz.tuple.permission.changed", "myelin://acme/issue/issue/X", serde_json::json!({}));
        assert!(matches!(ix.handle(&perm), HandleOutcome::NonRetryable(_)), "missing zookie/refs → poison");
    }

    // --- Removal / gone path ---

    /// **A `*.deleted`/`*.erased` event REMOVES the doc (and its co-located vector).** The `*.erased`
    /// tombstone re-drives this via the SAME live consumer (the SRCH-P15 erase path — no backdoor).
    #[test]
    fn delete_and_erase_remove_the_doc() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("body")));
        let ix = indexer_with(vec![issue_spec()], fetcher);
        ix.handle(&event("01J-1", "issue.issue.created", r, serde_json::json!({})));
        assert_eq!(ix.live_count(&tenant(), &region()), 1);

        // A `*.erased` removes it (the erasure path, through the live consumer).
        let erased = event("01J-e", "issue.issue.erased", r, serde_json::json!({ "ref": r }));
        assert_eq!(ix.handle(&erased), HandleOutcome::Done);
        assert_eq!(ix.live_count(&tenant(), &region()), 0, "the erased doc is removed from the index");
    }

    /// **An upsert event whose artifact PROJECTS AS GONE removes the doc (a clean removal, not a
    /// poison).** A re-index of a since-deleted artifact converges to "absent".
    #[test]
    fn upsert_of_a_gone_artifact_removes_the_doc() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("body")));
        let ix = indexer_with(vec![issue_spec()], fetcher.clone());
        ix.handle(&event("01J-1", "issue.issue.created", r, serde_json::json!({})));
        assert_eq!(ix.live_count(&tenant(), &region()), 1);

        // The owner now projects GONE (the artifact was deleted) — a re-index removes the doc.
        fetcher.projections.lock().unwrap().remove(r);
        assert_eq!(ix.handle(&event("01J-2", "issue.issue.updated", r, serde_json::json!({}))), HandleOutcome::Done);
        assert_eq!(ix.live_count(&tenant(), &region()), 0, "a gone projection removes the doc");
    }

    // --- The sub-artifact (#sub) path (5.7) ---

    /// **A sub-artifact doc (a `#sub`-anchored ref, 5.7) is keyed sub-precisely BUT pins its ACL on the
    /// `#sub`-stripped parent.** The owner resolved the sub-anchor as part of `project`; Search keys the
    /// doc by the full sub-URN and the ACL by the parent.
    #[test]
    fn sub_artifact_doc_pins_acl_on_the_parent() {
        let sub_ref = "myelin://acme/knowledge/page/42#block-9";
        let parent = "myelin://acme/knowledge/page/42";
        let fetcher = Arc::new(FakeFetcher::with(sub_ref, proj("a block of prose")));
        let ix = indexer_with(vec![page_spec()], fetcher);
        let ev = event("01J-b", "knowledge.page.created", sub_ref, serde_json::json!({}));
        assert_eq!(ix.handle(&ev), HandleOutcome::Done);

        // The doc is keyed by the full sub-URN; the ACL clause that admits it pins on the PARENT.
        let by_parent = ix
            .search_ft(&tenant(), &region(), &AclFilter::ids([parent]), "prose", 10)
            .expect("search by parent acl");
        assert_eq!(by_parent.len(), 1, "the sub-artifact doc is admitted by the PARENT's ACL (5.7/§3.1)");
        assert_eq!(by_parent[0].doc_id, sub_ref, "but keyed by the full sub-precise doc_id");
    }

    // --- Chained mutation: index → permission-change → re-index across a consumer restart (EI-01 §4) ---

    /// **CHAINED MUTATION (EI-01 §4 — chain, don't single-handler): index → permission-change →
    /// re-index across a SIMULATED CONSUMER RESTART, asserting exactly-once-in-effect.** A fresh
    /// `Consumer` re-binds the SAME dedup ledger (rule 4); the already-handled events are deduped (0
    /// dup), and the index reflects exactly the cumulative effect (one doc, advanced zookie, fresh body).
    #[test]
    fn chained_index_permchange_reindex_across_restart_is_exactly_once_in_effect() {
        use myelin_events::{
            Consumer, ConsumerName, DedupLedger, Delivered, Message, PrefetchBound, Subscription,
        };
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("first body")));
        let ix = indexer_with(vec![issue_spec()], fetcher.clone());
        let ledger = DedupLedger::new();
        let bind = || {
            Subscription::bind(ConsumerName(INDEXER_CONSUMER.into()), &["myelin://acme/"], PrefetchBound::DEFAULT)
                .unwrap()
        };
        let msg = |ev: &EventEnvelope| Message { subject: r.to_string(), envelope: ev.clone() };

        let e_index = event("01J-1", "issue.issue.created", r, serde_json::json!({ "zookie": "zk-1" }));
        let e_perm = event("01J-2", "authz.tuple.permission.changed", r, serde_json::json!({ "zookie": "zk-2", "refs": [r] }));

        // First connection: index, then permission-change. Then the broker "drops".
        {
            let c = Consumer::new(ix.clone(), bind(), ledger.clone());
            assert_eq!(c.deliver(&msg(&e_index)), Delivered::Acked);
            assert_eq!(c.deliver(&msg(&e_perm)), Delivered::Acked);
        }
        assert_eq!(ix.indexed_zookie_of(&tenant(), &region(), r).as_deref(), Some("zk-2"), "perm change advanced zookie");

        // The owner's body changes (a real re-index event re-fetches the fresh body).
        fetcher.put(r, proj("second body"));
        let e_reindex = event("01J-3", "issue.issue.updated", r, serde_json::json!({ "zookie": "zk-3" }));

        // Reconnect: a FRESH consumer re-binds the SAME ledger. The broker redelivers ALL three
        // (at-least-once); the two already-handled are deduped (0 dup), the new one re-indexes.
        let c2 = Consumer::new(ix.clone(), bind(), ledger.clone());
        assert_eq!(c2.deliver(&msg(&e_index)), Delivered::Deduplicated, "e_index already handled → 0 dup");
        assert_eq!(c2.deliver(&msg(&e_perm)), Delivered::Deduplicated, "e_perm already handled → 0 dup");
        assert_eq!(c2.deliver(&msg(&e_reindex)), Delivered::Acked, "the new re-index is handled → 0 lost");

        // EXACTLY-ONCE-IN-EFFECT: one doc, the fresh body, the latest zookie.
        assert_eq!(ix.live_count(&tenant(), &region()), 1, "exactly one doc (no dupe across restart)");
        let fresh = ix.search_ft(&tenant(), &region(), &AclFilter::ids([r]), "second", 10).expect("search");
        assert_eq!(fresh.len(), 1, "the re-index applied the fresh body");
        let stale = ix.search_ft(&tenant(), &region(), &AclFilter::ids([r]), "first", 10).expect("search");
        assert!(stale.is_empty(), "the old body was replaced (delete-then-add upsert)");
        assert_eq!(ix.indexed_zookie_of(&tenant(), &region(), r).as_deref(), Some("zk-3"), "latest zookie stamped");
    }

    // --- The transient-hiccup Retry path (0 lost) ---

    /// **A transient owner-fetch hiccup is a Retry (NOT acked) — a later redelivery re-runs and
    /// succeeds (0 lost, never a fabricated projection).** The runtime reverts the dedup mark on a
    /// Retry so the redelivery re-projects.
    #[test]
    fn transient_owner_hiccup_retries_then_succeeds() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("body")));
        fetcher.set_flaky(r, 1); // fail the FIRST project, succeed after.
        let ix = indexer_with(vec![issue_spec()], fetcher);
        let ev = event("01J-1", "issue.issue.created", r, serde_json::json!({}));

        // First handle: the owner is down → Retry (NOT a poison, NOT a fabricated empty doc).
        assert!(matches!(ix.handle(&ev), HandleOutcome::Retry(_)), "a transient hiccup retries");
        assert_eq!(ix.live_count(&tenant(), &region()), 0, "nothing indexed on the hiccup (no fabrication)");
        // Redelivery: the owner is back → Done, the doc indexes (0 lost).
        assert_eq!(ix.handle(&ev), HandleOutcome::Done, "the redelivery succeeds");
        assert_eq!(ix.live_count(&tenant(), &region()), 1, "0 lost: the doc indexed on the redelivery");
    }

    // --- Telemetry (contract 1.8 / §4.11) ---

    /// **`index_lag` (contract 1.8) returns to 0 in steady state, and its NAME is the named constant
    /// (drills assert the NAME, never a literal — no signal == failed drill).**
    #[test]
    fn index_lag_telemetry_is_zero_in_steady_state_and_named() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("body")));
        let ix = indexer_with(vec![issue_spec()], fetcher);
        assert_eq!(ix.index_lag(), 0, "a fresh indexer has no lag");
        ix.handle(&event("01J-1", "issue.issue.created", r, serde_json::json!({})));
        assert_eq!(ix.index_lag(), 0, "index_lag returns to 0 after projection (synchronous apply)");
        assert_eq!(IncrementalIndexer::INDEX_LAG_SIGNAL, "search.index_lag", "the contract-1.8 signal name");
    }

    /// **The IndexSpec shape is the synthetic-producer surface (6.3): semantic flag + the structured
    /// facets + the acl_object_type.** Pins the frozen shape (a producer registers to it).
    #[test]
    fn index_spec_shape_is_the_synthetic_producer_surface() {
        let s = issue_spec();
        assert_eq!(s.subsystem, "issue");
        assert_eq!(s.type_, "issue");
        assert_eq!(s.acl_object_type, "issue", "acl_object_type defaults to the type");
        assert!(!s.semantic, "issue is not semantically indexed");
        assert!(page_spec().semantic, "knowledge/page is semantically indexed");
        assert!(s.struct_fields.contains_key("status"));
    }
}

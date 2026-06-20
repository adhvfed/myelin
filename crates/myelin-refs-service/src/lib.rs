//! # `myelin-refs-service` — Refs as a `PersonalDataHolder` (stub surface) + the residency-pin
//! confirmation (REF-P3 / P-120)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/reference-graph.md` §3 (all Refs tables are
//! `(tenant, region)` first / RLS / **no cross-tenant query path**; every store is
//! residency-pinned, per-tenant envelope-encrypted, crypto-shred-capable, and a
//! `PersonalDataHolder` **auto-registered** by the harness — substrate §3.4 / contract 1.4),
//! §3.6 (the projection cache is itself a bounded, invalidatable `PersonalDataHolder`), §4.6 (the
//! small, structural erasure surface: `locate(subject)` → edges/cache entries naming the subject,
//! `erase(subject)` → purge R2 cache PII + rely on Identity's pseudonym-map shred for
//! `origin_actor`; Refs **never holds the PII itself** for the references-not-payloads case).
//!
//! **Reconciliation:** `00-reconciliation-decisions.md` **X-7** (the ONE platform-wide
//! free-text/immutable erasure posture, contract 10.9) — Refs instantiates this posture **by
//! reference** and adds **no new `[OPEN — LEGAL]` residual**: its only personal data is
//! pseudonymous opaque ids (`origin_actor`) + cache titles, never third-party free-text bodies.
//!
//! **Contract-index rows:**
//! - **10.1** `PersonalDataHolder{locate, export, rectify, restrict, erase}` — IMPLEMENTED here as
//!   holder **H12 (`ReferenceGraph`)** to the frozen `myelin_gdpr` shape. At M1 it is a STUB:
//!   the edge index + R2 cache do not exist yet, so `locate`/`export` return **empty-but-correct**
//!   results (a tenant with no edges has no located data), and `restrict`/`erase` are well-defined
//!   no-ops returning a content-addressed receipt. The REAL erase (purge R2-cache PII + reindex
//!   tombstones) lands in **REF-P15** once the index exists.
//! - **1.4** the harness holder **auto-registration** — Refs registers its (future) stores through
//!   the substrate [`myelin_substrate::HolderRegistry`] so the H1–H18 holder list is exhaustive
//!   BEFORE any tenant data exists. The substrate completeness assertion
//!   ([`myelin_substrate::assert_holder_completeness`]) confirms every store Refs opens classifies
//!   to [`myelin_substrate::Holder::H12ReferenceGraph`] — 0 orphan stores.
//! - **12.x** the residency-pin — CONFIRMED structurally: every Refs store is
//!   `(tenant, region)`-partitioned, carries a [`myelin_tenancy::ResidencyTag`], and has no
//!   cross-tenant query path. The `residency-pin` + `tenant-predicate` lints (REF-P2) enforce this;
//!   this crate LINKS them by threading `myelin_tenancy::{TenantId, Region, ResidencyTag}` through
//!   its store descriptors (the token types the lints recognise).
//!
//! ## What REF-P3 (P-120) + REF-P4 (P-121) ship — and what they deliberately do NOT (VISION §3)
//! **REF-P3 ships:** the [`holder`] module — Refs as a real, registered `PersonalDataHolder` (H12)
//! over its two (future) stores (the edge OLTP index + the R2 projection cache), each registered
//! through the substrate holder registry; the [`residency`] module — the `(tenant, region)` +
//! residency-tag store descriptors that confirm the residency-pin applies + link the residency-pin
//! lint; the [`erasure_posture`] record — Refs adds NO new free-text residual (X-7 by reference).
//!
//! **REF-P4 (P-121) ships:** the [`dek`] module — the Refs **per-tenant DEK** reserved in the cell's
//! ONE KMS hierarchy ([`myelin_storage::KmsEngine`], 11.3 / 11.4) so the (future) edge index + R2
//! cache are **encrypted-from-birth**, with **destroy callable** on the key class (the
//! tenant-decommission crypto-shred lever) + the **per-subject DEK backstop** (§3.6, "a name in a
//! cached title") + the inherited-M1-gate precondition list named for REF-P5
//! ([`dek::ref_p5_inherited_gates`]).
//!
//! **REF-P5 (P-154) ships:** the [`migration`] module — the **edge inverse-index schema migration**
//! (the §3.2 `edge` table + its three indexes `edge_inbound`/`edge_outbound`/`edge_by_rel`), as a
//! **forward-only online migration** (contract 1.5) through the substrate framework, **RLS-on**
//! (the platform `myelin_make_tenant_scoped` convention), **`(tenant, region)`-first**, and
//! **encrypted-from-birth** under the REF-P4 per-tenant DEK ([`edge_table_dek_ref`]). The live-DB
//! apply + RLS isolation + the three indexes are proven against the dev stack in
//! `tests/integration_ref_p5_edge_schema.rs` (the `integration` feature). This is the SCHEMA ONLY —
//! the builder/invalidator that POPULATE it are **REF-P6/P7**.
//!
//! **REF-P6 (P-155) ships:** the [`edge_builder`] module — the **refs-edge-builder consumer**
//! (contract 5.4 consumer side): an ordinary [`myelin_events::EventHandler`] that whitelists
//! `refs.edge.>` + the typed-lifecycle subjects `issue.relation.>` / `knowledge.page.>` (NEVER `*` —
//! a reviewed firehose-class consumer, BUS-4), **upserts on `*.created`** (idempotent via the
//! deterministic [`edge_builder::edge_id`] = `hash(tenant, source, target, rel)`), **tombstones on
//! `*.removed`/`*.erased`** (§4.6), writes `source_root`/`target_root` by [`myelin_refs::strip_sub`],
//! and emits the **`refs.index_lag`** telemetry (contract 1.8). **Steady-state == cold-rebuild is ONE
//! code path** ([`edge_builder::RefsEdgeBuilder::project`]) — a live event and a reindex-from-source
//! `*.snapshot` replay both flow through it, with NO owner-DB backdoor (the no-cross-db floor,
//! REF-D4). The REF-D7 ingest half (0 ghost / 0 lost, emit-iff-committed) + the idempotent rebuild
//! upsert are proven against the live dev-stack Postgres in `tests/integration_ref_p6_edge_builder.rs`
//! (the `integration` feature). The in-memory [`edge_builder::EdgeProjection`] models the §3.2 `edge`
//! table; the REAL `INSERT … ON CONFLICT` against the per-tenant-DEK-encrypted table (executed in the
//! SAME tx as the `consumer_dedup` mark) lands when the OLTP store is wired into `serve`.
//!
//! **REF-P7 (P-156) ships:** the [`invalidator`] module — the **refs-projection-invalidator
//! consumer** (§4.3 second consumer): an ordinary [`myelin_events::EventHandler`] that whitelists the
//! `*.updated`/`*.erased` lifecycle subjects (NEVER `*` — a reviewed BUS-4 firehose-class consumer)
//! and **busts the projection cache per `ArtifactRef`** (the §3.6 `(tenant, ref)` bust) through the
//! [`invalidator::ProjectionCache`] invalidation INTERFACE, idempotent on `event_id` via
//! `consumer_dedup` (2.4/2.5). Because the live R2 cache lands in **REF-P12**, this prompt ships the
//! interface plus a **[`invalidator::NoOpCacheShim`]** behind it — the shim holds nothing but RECORDS
//! every bust call, so the consumer's behaviour is observable + testable (one `invalidate(tenant,
//! ref)` per `*.updated`/`*.erased`). REF-P12 replaces the shim with the live bounded cache by
//! implementing the SAME trait — the consumer is unchanged. Telemetry `refs.invalidations`
//! ([`invalidator::RefsProjectionInvalidator::INVALIDATIONS_SIGNAL`]) is live.
//!
//! **REF-P8 (P-157) ships:** the [`emit`] module — the **edge-extraction emit seam** (contract 5.4
//! EMIT side, the §4.1 producer #1): given a `myelin-content` document (the three structured inline
//! nodes [`myelin_content::InlineNode`] — `mention`/`artifact_ref`/`embed`, frozen X-2), [`extract_edges`]
//! yields **one edge per structured node** (`mention → mentions`, `artifact_ref → links`,
//! `embed → embeds`; `rel_class = reference`) by **matching the enum variant** — structured-node
//! extraction, NOT a regex over prose (the reliability guarantee, EI-04 §2.4). [`emit_edges`] emits one
//! `refs.edge.created` per edge in the **SAME transaction** that writes the content, via
//! [`myelin_events::OutboxTx::emit`]`(draft, cause = Some(content_event))` — so the **correlation root
//! carries**, `causation = the content event`, and `depth = content.depth + 1` (the loop-guard stamp,
//! the explicit drill REF-P9). There is **NO standalone edge-write API** (the only verb is
//! `OutboxTx::emit`; the `no-raw-publish` lint, P-019, stays green). **Emit-iff-committed** (REF-D7
//! producer half, BUS-D4): the edge rows are BUFFERED into the content transaction and become durable
//! **iff** the caller commits — abort → 0 edges (no edge without its content). Proven against the live
//! dev-stack outbox in `tests/integration_ref_p8_emit_seam.rs` (the `integration` feature: N nodes
//! committed → N rows visible to the relay; aborted → 0). The mention's target is the PSEUDONYMOUS
//! `member` URN, never the name (erasure-safe, §4.6).
//!
//! **REF-P9 (P-158) ships:** the [`loop_guard`] module — the **loop-guard causal-depth stamp** on
//! every `refs.edge.*` emit (§4.1 `depth +1`; AG-6). [`RefsLoopGuard::guarded_emit_edges`] wraps the
//! REF-P8 emit seam and (1) STAMPS every emitted `refs.edge.*` at [`stamped_depth`] =
//! `content.depth + 1` (the explicit drill REF-P8 deferred — the `+1` rides
//! [`myelin_events::derive_envelope`] correct-by-construction, now ASSERTED through the real outbox);
//! (2) gates the **re-trigger source** to structured `artifact_ref`/`embed` nodes only
//! ([`is_retrigger_source`] — a `mention` notifies, never auto re-triggers; AG-6 / CHAT-1); and
//! (3) PARKS the emit + fires a **depth-ceiling tripwire** ([`RefsLoopGuard::ceiling_tripwire_firings`])
//! when a chain reaches the frozen AG-6 causal ceiling ([`CAUSAL_DEPTH_CEILING`] = 12, DISTINCT from
//! the Refs traversal ceiling 16, REF-P13) — **before runaway**, so the deepest edge ever written
//! sits at `ceiling - 1`. The guard FEEDS the **causal-depth telemetry** (`bus.causal_depth_max`,
//! contract 1.8 / [`myelin_events::BusSignal::CausalDepthMax`]) so an approaching chain is observable.
//! The mutation floor is on the stamp/ceiling logic (leak-of-runaway-critical): the saturating `+1`,
//! the `>= ceiling` park boundary, and the re-trigger gate are each asserted (a mutant that wraps the
//! depth, parks one hop late, or treats a mention as a re-trigger is caught). There is NO second
//! causality function — the `+1` is still `derive_envelope`; only the ceiling NUMBER is re-stated
//! (refs-service must not depend on the mid-tier query crate; DOCUMENTED in [`loop_guard`]).
//!
//! **REF-P11 (P-160) ships:** the [`backlinks`] module — the **permission-filtered backlink read**
//! (contract 5.3 OWNED): [`backlinks::BacklinkRead::backlinks`]`(target, viewer, page)` +
//! [`backlinks::BacklinkRead::edges`]`(ref, viewer)` lower the FROZEN `list_objects` `SetExpr`
//! (consumed contract 4.3) over the §3.2 `edge.source_root` column (C-4) — `Ids`/`NotIds` →
//! `IN`/`NOT IN`, `InRelation`/`TupleSet` → JOIN the per-tenant residency-pinned `authz_visible`
//! reverse index, `Union`/`Intersect`/`Difference` → `AND`/`OR`/`EXCEPT`, `All` → no predicate,
//! `None` → `WHERE false` — into **ONE** query with **NO N+1** and **NO post-filter**, always
//! paginated, carrying `WHERE tenant = :viewer.tenant` (no cross-tenant path). The carried zookie
//! drives the new-enemy guard (4.10): a reverse index BEHIND the zookie's required revision falls
//! back to per-source `check` rather than serving a stale grant ([`backlinks::watermark_verdict`]).
//! The query-count (no-N+1) + filter-mode-split (`Ids` vs pushed-down) telemetry fire (1.8). Refs is
//! one of the five named `SetExpr` consumers — it shares the FROZEN `myelin_identity::SetExpr` enum
//! (the CONTRACT crate both consume), lowering it over its OWN id column; **no Id signature change**,
//! and **no dep on identity-service** (the identity-side lowering is a sibling leaf service; the
//! algebra is restated over `source_root`, the SHAPE pinned identical by tests — DOCUMENTED in
//! [`backlinks`]). REF-D1 (backlink half: 0 leak), REF-D2 (0 cross-tenant), REF-D6 (no stale allow)
//! are greened in unit + chained + drill tests; the REAL SQL conjoin (the lowered predicate ANDed
//! into the `edge_inbound` scan with the live `authz_visible` JOIN) is PROVEN against the live
//! dev-stack Postgres in `tests/integration_ref_p11_backlink_setexpr.rs` (the `integration` feature).
//! **FLOOR named:** the read-time scan + filter + pagination is the hot-artifact floor; the
//! Leopard-style flattened reach index **R4** (the follow-on, promoted at measured hot-fanout > read
//! budget) is **REF-P23** (R-M5) — "we page them, we don't materialise them" is not the at-scale
//! answer.
//!
//! **REF-P12 (P-161) ships:** the [`cache`] module — the **live R2 projection cache**
//! ([`cache::R2ProjectionCache`], §3.6) that **REPLACES the REF-P7 no-op shim**. A bounded,
//! invalidatable, per-tenant-DEK-encrypted, residency-pinned holder keyed `(tenant, ref)`, riding the
//! ONE [`myelin_storage::Cache`] primitive (the [`myelin_storage::InMemoryCache`] floor for unit tests
//! / the `ValkeyCache` real backing behind `--features integration`). It implements BOTH seams the
//! REF-P7/REF-P10 floors stubbed — the write/invalidate side
//! ([`invalidator::ProjectionCache`] — the REF-P7 [`invalidator::NoOpCacheShim`] is replaced, an
//! `*.updated`/`*.erased` now EVICTS a live entry) AND the read side
//! ([`resolve::ProjectionCacheRead`] — the REF-P10 [`resolve::NoOpCacheRead`] is replaced, a warm
//! `(tenant, ref)` now serves a live HIT). The resolve chokepoint FILLS the cache after a miss
//! ([`cache::R2ProjectionCache::fill`], §4.2) so the next viewer is served a HIT (viewer-independent,
//! ref-keyed, gated by the per-viewer check). The cached projection — which **may hold a name in a
//! title** (§3.6) — is **sealed under the per-tenant DEK** (REF-P4; 11.3/11.4), so it is
//! encrypted-at-rest + **crypto-shred-able** (destroy the DEK → every cached title unrecoverable; a
//! decrypt-fail is a clean MISS, never a plaintext fall-through). Every write carries a **TTL**
//! ([`cache::R2_DEFAULT_TTL`]) — the cache self-evicts, so it is **NEVER a source of truth** (on a
//! miss/bust/erasure it re-resolves via the owner's `project`). The `resolve_cache_hit_ratio` telemetry
//! (1.8) is LIVE — it now reads real hits. The chained (hit → `*.updated` → miss → re-resolve through
//! the chokepoint) + the never-serve-stale-on-erasure + the crypto-shred CDC tests pass DB-free
//! (`tests/cdc_ref_p12_r2_cache.rs`); the REAL Valkey round-trip (fill/read/bust/crypto-shred/tenant-
//! isolation) is proven against the live dev stack in `tests/integration_ref_p12_r2_cache.rs` (the
//! `integration` feature). **FLOOR named:** the cache holds PII (a name in a title) sealed under the
//! per-tenant DEK; the SUBJECT-grain structural ERASE that drives the holder `erase` body (purge the
//! subject's cached titles + Identity pseudonym shred for `origin_actor` + reindex-from-source) lands
//! in **REF-P15** — the cache is the crypto-shred-able holder, NOT the complete erasure answer.
//!
//! **REF-P13 (P-162) ships:** the [`traverse`] module — the **bounded, cycle-safe recursive-CTE
//! traverse** ([`traverse::Traverse::traverse`], contract 5.3 OWNED; §4.5): a `WITH RECURSIVE`-shaped
//! BFS over the §3.2/§3.4 `edge` adjacency list ([`edge_builder::EdgeProjection::outbound_live`])
//! filtered by `rel`/`rel_class` ([`traverse::TraverseFilter`]), with a `path`-array **visited-set
//! cycle guard** (a self-referential graph TERMINATES, the cycle surfaced as a
//! [`traverse::TraverseResult::cycle_detected`] DIAGNOSTIC, never a hang — drill REF-D8), a **depth
//! ceiling** (default 16, read from the thresholds file `[refs_traverse]` — the single source of
//! truth, [`traverse::TRAVERSE_DEPTH_CEILING`]; DISTINCT from the agent CAUSAL ceiling 12), a
//! **collected-node budget** (X-3 — a wide graph the depth ceiling alone would not bound), a
//! **PARTIAL result + `truncated` marker** when either budget is hit (never an unbounded scan), and
//! **ONE** `list_objects` post-filter over the COLLECTED node set — **NOT per-hop**
//! ([`traverse::apply_post_filter`]) — where a hop into an unreadable artifact **PRUNES that branch**
//! (the node AND everything reachable only through it are dropped — the traversal is not a
//! side-channel; drill REF-D1 traverse half: 0 leak). The post-filter reuses the SAME FROZEN
//! [`backlinks::set_expr_admits`] the backlink read lowers (ONE source of truth, no second algebra).
//! **FLOOR named:** the `rel_class`/lifecycle edges the traverse walks are minted as a DISCIPLINED,
//! inverse-paired vocabulary by the **TE-7 mirror discipline (REF-P14)** — named so a `blocked_by`
//! traverse is not mistaken for cross-subsystem-lifecycle-aware before the mirror discipline lands
//! (the walk MECHANISM is real here; the lifecycle VOCABULARY is REF-P14). The statement-timeout +
//! the real Postgres `WITH RECURSIVE … CYCLE … LIMIT` over the per-tenant-DEK-encrypted `edge` table
//! land with the OLTP store in `serve` (REF-P5+); the BOUND DISCIPLINE is real + proven over the
//! in-memory adjacency model here.
//!
//! **Does NOT ship (floors named):**
//! - **The producers are SYNTHETIC at M2.** REF-P8 exercises the seam with a TEST content writer; the
//!   first REAL producers (Git diffs / Knowledge blocks / Chat messages writing actual content) land
//!   in **REF-P17 / REF-P18** (M3+). Named so the seam is not mistaken for live producer edges — the
//!   WIRING (structured-node extraction + same-tx outbox emit) is real, the CALLERS are synthetic.
//! - **The loop-guard causal-depth STAMP drill is REF-P9 / P-158.** The `depth = content.depth + 1`
//!   already rides [`myelin_events::derive_envelope`] correct-by-construction (the emit passes `cause =
//!   Some(content_event)`); REF-P9 adds the explicit depth-stamp assert + the depth-ceiling tripwire
//!   over THIS seam.
//! - **The LIVE R2 cache HAS LANDED (REF-P12 / P-161).** The REF-P7 [`invalidator::NoOpCacheShim`] +
//!   the REF-P10 [`resolve::NoOpCacheRead`] are no longer the only impls of the cache seams: the live
//!   [`cache::R2ProjectionCache`] now implements both, sealed under the per-tenant DEK + TTL-bounded +
//!   crypto-shred-able (the floor named in REF-P7 is RESOLVED here). The no-op shims REMAIN as the
//!   floor/default impls (a `serve` that has not wired Valkey, and the `ProjectionCacheRead::fill`
//!   default) — the invalidator/resolve are unchanged; only the trait object behind them swapped, as
//!   REF-P7/P10 promised. The REAL cache mutation floor (keying/sealing/invalidation under TTL +
//!   crypto-shred) IS met here ([`cache`] mutation-score floor; see the module doc + the COMMIT body).
//! - **No real crypto-shred over real data.** The holder is a STUB surface: `erase` is a
//!   well-defined no-op now (nothing to purge). The DEK lever EXISTS + FIRES (proven structurally in
//!   [`dek`]), but the structural erasure body that USES it — R2-cache PII purge + reliance on
//!   Identity's pseudonym shred for `origin_actor` + `*.erased` tombstoning — lands in **REF-P15**
//!   (M2); the world-scale 0-recoverable shred drill (REF-D5) is **REF-P15 / REF-P25**.
//! - **The holder is registered + the DEK reserved, but no store is OPENED at runtime here.** `serve`
//!   opens the real stores (auto-registering them + wiring the [`dek::RefsDekPin`] into them) when
//!   the edge schema lands (REF-P5+). This crate proves the registration + classification + DEK pin
//!   are correct so the M5 DSAR fan-out cannot silently miss Refs and the index is encrypted-from-birth.
//!
//! So this crate at M1 is the holder REGISTRATION + the residency-pin CONFIRMATION + the per-tenant
//! DEK PIN — not the engine, not the real erasure.

#![forbid(unsafe_code)]

pub mod backlinks;
pub mod cache;
pub mod dek;
pub mod edge_builder;
pub mod emit;
pub mod erasure_posture;
pub mod holder;
pub mod invalidator;
pub mod loop_guard;
pub mod migration;
pub mod residency;
pub mod resolve;
pub mod traverse;

pub use backlinks::{
    ids_result, lower_over_source_root, set_expr_admits, source_root_colref, view_permission,
    watermark_verdict, AuthzJoin, AuthzVisibleIndex, Backlink, BacklinkError, BacklinkPage,
    BacklinkRead, BoundParam, FilterMode, SourceRootFilter, WatermarkVerdict,
    AUTHZ_VISIBLE_TABLE, FILTER_MODE_SPLIT_SIGNAL, SOURCE_ROOT_COLUMN,
};
pub use cache::{
    CacheFillError, R2ProjectionCache, R2_DEFAULT_TTL, R2_KEY_PREFIX,
};
pub use dek::{ref_p5_inherited_gates, InheritedGate, RefsDekPin};
pub use edge_builder::{
    edge_id, EdgeProjection, EdgeRow, ProjectError, RefsEdgeBuilder, RelClass,
    EDGE_BUILDER_CONSUMER, EDGE_BUILDER_SUBJECTS, EDGE_BUILDER_SUBJECT_PREFIXES,
};
pub use emit::{
    edge_aggregate_key, emit_edges, extract_edges, EdgeDraft, EdgeRel, REFS_EDGE_CREATED,
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
pub use erasure_posture::{erasure_posture, ErasurePosture};
pub use invalidator::{
    InvalidateError, InvalidationCall, NoOpCacheShim, ProjectionCache, RefsProjectionInvalidator,
    INVALIDATOR_CONSUMER, INVALIDATOR_SUBJECTS, INVALIDATOR_SUBJECT_PREFIXES,
};
pub use holder::{
    refs_store_classifier, register_refs_holders, RefsCacheHolder, RefsEdgeHolder,
    RefsHolderRegistration, REFS_CACHE_STORE, REFS_EDGE_STORE,
};
pub use residency::{refs_store_descriptors, RefsStoreDescriptor};
pub use traverse::{
    apply_post_filter, depth_ceiling_from_thresholds, max_nodes_from_thresholds, Traverse,
    TraverseFilter, TraverseNode, TraverseResult, TRAVERSE_DEPTH_CEILING, TRAVERSE_MAX_NODES,
};
pub use resolve::{
    bounded_stale, strong_read, AuthzServed, CrossCellDisposition, NoOpCacheRead, OwnerProjection,
    ProjectApi, ProjectApiError, ProjectOutcome, Projection, ProjectionCacheRead, ProjectionFlag,
    ResolveMode, ResolveService, Resolution, Tombstone, TombstoneReason,
    RESOLVE_CACHE_HIT_RATIO_SIGNAL, VIEW_PERMISSION,
};

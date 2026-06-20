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
//! **Does NOT ship (floors named):**
//! - **No INVALIDATOR, no R2 cache.** REF-P6 ships the BUILDER (ingest); the `*.updated`/`*.erased`
//!   cache invalidation it would drive is **REF-P7**'s refs-projection-invalidator (over the no-op
//!   cache shim); the live R2 cache is **REF-P12**. Nothing here busts a cache yet — ingestion is not
//!   a live projection.
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

pub mod dek;
pub mod edge_builder;
pub mod erasure_posture;
pub mod holder;
pub mod migration;
pub mod residency;

pub use dek::{ref_p5_inherited_gates, InheritedGate, RefsDekPin};
pub use edge_builder::{
    edge_id, EdgeProjection, EdgeRow, ProjectError, RefsEdgeBuilder, RelClass,
    EDGE_BUILDER_CONSUMER, EDGE_BUILDER_SUBJECTS, EDGE_BUILDER_SUBJECT_PREFIXES,
};
pub use migration::{
    edge_ddl_is_forward_only, edge_table_dek_ref, edge_table_migrations, CREATE_EDGE_INDEXES_DDL,
    CREATE_EDGE_TABLE_DDL, EDGE_BY_REL_INDEX, EDGE_INBOUND_INDEX, EDGE_MIGRATION_ID,
    EDGE_OUTBOUND_INDEX, EDGE_TABLE, MAKE_EDGE_TENANT_SCOPED_DDL,
};
pub use erasure_posture::{erasure_posture, ErasurePosture};
pub use holder::{
    refs_store_classifier, register_refs_holders, RefsCacheHolder, RefsEdgeHolder,
    RefsHolderRegistration, REFS_CACHE_STORE, REFS_EDGE_STORE,
};
pub use residency::{refs_store_descriptors, RefsStoreDescriptor};

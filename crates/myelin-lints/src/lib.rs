//! # `myelin-lints` — the committed architecture-lint ratchet (the twelve lints)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.11 (the enforced-architecture-lint table). **Contract-index:** row 1.6 (the twelve
//! architecture lints; each ships a red fixture that proves it rejects + a green fixture that
//! proves it admits). **Doctrine:** `external-insights/01-process-and-quality-doctrine.md` §5
//! (the ratchet — convert each discipline into a committed, mechanical, LOUD gate; an
//! uncommitted gate is no gate; replace `... || true` — a violation is a typed `Err`, never a
//! swallowed pass).
//!
//! ## What this crate ships (P-S10 → P-017, then P-S11 → P-018: the full TWELVE)
//! ALL TWELVE architecture lints from the §2.11 table, as `cargo`-level source-scanning checks,
//! each with a red + a green fixture. The FOUR most load-bearing (P-S10 → P-017):
//! - [`tenant_predicate`] — every query-builder call carries a `TenantId` bound; a tenant-less
//!   query is rejected (F2, the IDOR floor; EI-02 §1, ID-3).
//! - [`no_raw_publish`] — no bus publish outside `OutboxTx::emit`; there is NO fire-and-forget
//!   `publish_now` path (F5; BUS-2).
//! - [`no_host_exec`] — no host-execution path (`std::process::Command`, `tokio::process`, a raw
//!   `exec`) bypassing `ToolHands::exec` = the unified sandbox (X-6; AG-2).
//! - [`no_untagged_personal_data`] — every schema field carrying PII is `#[personal_data(...)]`
//!   -tagged; an untagged PII column is rejected (ADR-12; recon §10.2).
//!
//! The REMAINING EIGHT, completing the twelve (P-S11 → P-018):
//! - [`no_cross_db`] — a service crate must not depend on another service's storage module (ADR-01).
//! - [`forward_only_migration`] — no rollback migration file; no blocking `ALTER` on a hot table
//!   (STOR-2, §9).
//! - [`no_cross_sync_cycle`] — the sync call graph is acyclic; Identity is a sink (EI-02 §3).
//! - [`residency_pin`] — every store/stream/index/cache declares a region; no global pool (ADR-11).
//! - [`control_plane_pii_free`] — the control plane carries opaque ids only (ADR-11; recon §OQ-I).
//! - [`search_requires_acl_filter`] — every search/list query pre-filters on the ACL `Filter`
//!   before scoring (ADR-03; recon §OQ-E).
//! - [`no_llm_in_platform`] — no LLM SDK / prompt / model name in platform code; the runtime is
//!   behind the `AgentRuntime` strategy seam (ADR-08.2; VISION §3).
//! - [`flow_determinism`] — a `myelin-flow` workflow body uses only the deterministic `WfCtx`
//!   surface (index 9.2; recon §OQ-F).
//!
//! These twelve make whole bug-classes impossible to ship: a tenant-less query (cross-tenant
//! IDOR), a fire-and-forget publish (a lost event / a causality break), a sandbox-bypassing
//! host exec (a privilege escape), an untagged PII column (an un-erasable / un-mapped subject), a
//! cross-DB reach, a reversible / table-locking migration, a sync cycle, a region-less store, a
//! PII-carrying control-plane frame, a post-filter search leak, an LLM SDK in platform code, and
//! a non-deterministic workflow body are each caught BEFORE they merge.
//!
//! ## Why a source-scanner (the chosen mechanism + its named floor)
//! The §2.11 lints are specified as "`cargo`-level architecture tests / clippy-style checks".
//! A full procedural-macro / `rustc_driver` clippy plugin would couple the gate to a pinned
//! nightly toolchain (fragile, non-hermetic). Instead this crate ships a **hermetic,
//! deterministic source-scanning engine** (pure string analysis, no toolchain, no DB, no
//! network): each lint is a [`Lint`] whose [`Lint::scan`] reads a unit of source text and
//! returns typed [`Violation`]s. The engine is wired LOUD two ways:
//!   1. the **fixture matrix** (`tests/fixture_matrix.rs`) runs each lint's red fixture and
//!      asserts ≥1 violation (4/4 reject), each green fixture and asserts 0 violations (4/4
//!      admit) — this matrix IS the test (P-S10 TESTS field);
//!   2. the **workspace scan** (`tests/workspace_clean.rs`) runs all twelve lints over Myelin's
//!      OWN `crates/*/src` tree and fails the build on any violation — the gate is live on real
//!      code, not just fixtures (EI-01 §5, "an uncommitted gate is no gate").
//!
//! **Floor named — scanner-grade, not type-system-grade.** The §2.11 ideal for
//! `tenant-predicate` / `no-untagged-personal-data` is "fails to *compile*" (a type-system
//! guarantee). This crate ships the *committed, loud, regression-tested* scanner form now so
//! the gate is live in M0; the type-system tightening (a `TenantId`-bound query-builder type
//! that makes a tenant-less query a compile error, a `#[personal_data]` derive that refuses to
//! expand an untagged PII column) lands with the query-builder (Identity/Storage M1) and the
//! classify-derive macro (**P-GA-07 / P-107**) respectively. The scanner is the ratchet's first
//! click; the macro is the second — the lint is never weakened, only sharpened.
//!
//! ## The remaining-eight floors (P-S11 → P-018: live-now, tighten-on-consumer)
//! Four of the eight new lints target code that does NOT exist yet. Per the P-S11 DELIVERABLE the
//! lint + its fixtures ship NOW (the gate is live before the consumer) and each is NAMED as a
//! floor that tightens when the targeted surface lands:
//! - `forward-only-migration` — the per-table hot-table half tightens when the hot-table
//!   declaration + migration runner land (**P-S15 / P-032**); the table-independent half (no down
//!   migration, no blocking `ALTER ... NOT NULL`) is enforced now. **P-ST-04 / P-020** adds the
//!   STORAGE-relevant red+green fixtures (the in-place-rewrite / contract-before-backfill bug shape
//!   vs the online expand→backfill→contract shape) in `tests/storage_lints.rs`; the migration
//!   RUNNER itself is **P-ST-05 / P-048**.
//! - `residency-pin` — tightens (adds each concrete store constructor) as the OLTP/blob/index
//!   stores land (**P-ST-01 / P-007**, **P-ST-03 / P-047**, …). **SHARPENED in P-ST-04 / P-020:**
//!   the fingerprint set now constrains the REAL OLTP constructors
//!   (`OltpPool::open(` / `ColocatedOltp::open(`); a region-less caller open is rejected. The M0
//!   pool MODEL is region-less by named design (the cell region pins via the per-query
//!   `(tenant, region)` `TenantScope`; the per-pool runtime region-pin is the M1 follow-on
//!   **P-ST-15 / P-102**, STOR-D5), recorded LOUDLY via the `@residency-cell-pinned` waiver marker.
//!   Storage's twin fixtures + the CI-wiring proof live in `tests/storage_lints.rs`.
//! - `control-plane-pii-free` — **SHARPENED in P-CP-04 / P-028** (Tenancy ownership): added the
//!   DATA-MAP LEG — a control-plane field classified `is_personal=true` (tagged
//!   `#[personal_data(...)]`, the generated data-map, contract 10.2) fires the lint regardless of
//!   its NAME, realizing the canonical §4.3 rule "no control-plane registry column is
//!   `is_personal=true`". The P-S11 name-fingerprint leg is kept as defence-in-depth. Tenancy's twin
//!   fixtures over the real frozen `CrossCellPointer` frame (P-CP-02 / P-027) + the CI-wiring proof
//!   live in `tests/tenancy_control_plane_lints.rs`. The live registry-schema CP-D1 drill is the M1
//!   follow-on **P-CP-05 / P-080**.
//! - `search-requires-acl-filter` — tightens to the type-system form when the permission-aware
//!   query pipeline lands (**SRCH-P08 / P-171**); Search ships its own twin in **SRCH-P01 / P-021**.
//! - `flow-determinism` — re-shipped against the real `WfCtx` in **P-FLOW-08 / P-200** when the
//!   `myelin-flow` crate lands (**P-FLOW-04 / P-199**).
//! - `no-llm-in-platform` — the `AgentRuntime` strategy seam (the one legitimate SDK site, excluded
//!   by name) lands in **AG-P1 / P-130**.
//! - `no-cross-sync-cycle` already has a build-layer twin (the `crate-graph-acyclic` test in
//!   `myelin-substrate`); this crate ships the source-scanning form (the Identity-sink invariant).
//!
//! The lints are never weakened — only sharpened — as each consumer lands (EI-01 §5).
//!
//! ## The contract-coverage scanner — the META-gate (P-S21 → P-037) — see [`coverage`]
//! Sibling to the twelve lints: where a lint catches a bug-class in code, the
//! [`coverage`] scanner catches the bug-class one level up — a cross-system CONTRACT that ships
//! without its provider+consumer CDC test pair. It reconciles the frozen contract-index against the
//! workspace-root `contract-coverage.toml` manifest and FAILS LOUDLY (the `contract-coverage` bin's
//! exit code) on any falsely-claimed or silently-dropped row. **DEVIATION (EI-01 §1,
//! coherence-over-slavish-plan-following):** the P-S21 DELIVERABLE offers "a CI tool in
//! `myelin-substrate` OR a workspace `xtask`". It is placed HERE in `myelin-lints` instead, because
//! `myelin-lints` is already the established committed-gate SOURCE-SCANNER LEAF (it sits OUTSIDE the
//! production crate DAG, architecture §2.9; nothing depends on it). Putting a repo-scanning build
//! tool into `myelin-substrate` — the production ROOT crate every service's `main` links — would
//! pull Markdown/TOML-scanning build machinery into the shipped binary surface. The scanner is the
//! conceptual sibling of the `lint-gate` binary (both are the EI-01 §5 ratchet), so it lives beside
//! it. The deliverable's INTENT (a committed, loud, CI-wired meta-gate) is met exactly; only the
//! host crate differs, and it differs toward the more coherent home.

pub mod coverage;
pub mod engine;
pub mod lints;
pub mod production_graph;

pub use coverage::{
    parse_contract_index_rows, parse_manifest, scan, Coverage, CoverageError, ManifestEntry,
    PairEvidence, RowId, ScanReport,
};

pub use engine::{Lint, LintId, Violation};
pub use production_graph::{
    no_bare_tenant_pool, no_in_memory_durable_store, no_permissive_authorizer_in_prod,
    no_structural_crypto_in_prod, production_graph_absence_scanners, NO_BARE_TENANT_POOL,
    NO_IN_MEMORY_DURABLE_STORE, NO_PERMISSIVE_AUTHORIZER_IN_PROD, NO_STRUCTURAL_CRYPTO_IN_PROD,
    PRODUCTION_GRAPH_ABSENCE_SCANNERS,
};
pub use lints::{
    all_twelve, control_plane_pii_free, flow_determinism, forward_only_migration,
    load_bearing_four, no_cross_db, no_cross_sync_cycle, no_host_exec, no_llm_in_platform,
    no_raw_publish, no_untagged_personal_data, remaining_eight, residency_pin,
    search_requires_acl_filter, tenant_predicate, ALL_TWELVE, LOAD_BEARING_FOUR, REMAINING_EIGHT,
};

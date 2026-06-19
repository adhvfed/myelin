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
//! ## What this crate ships (P-S10 → P-017)
//! The FOUR most load-bearing architecture lints from the §2.11 table, as `cargo`-level
//! source-scanning checks, each with a red + a green fixture:
//! - [`tenant_predicate`] — every query-builder call carries a `TenantId` bound; a tenant-less
//!   query is rejected (F2, the IDOR floor; EI-02 §1, ID-3).
//! - [`no_raw_publish`] — no bus publish outside `OutboxTx::emit`; there is NO fire-and-forget
//!   `publish_now` path (F5; BUS-2).
//! - [`no_host_exec`] — no host-execution path (`std::process::Command`, `tokio::process`, a raw
//!   `exec`) bypassing `ToolHands::exec` = the unified sandbox (X-6; AG-2).
//! - [`no_untagged_personal_data`] — every schema field carrying PII is `#[personal_data(...)]`
//!   -tagged; an untagged PII column is rejected (ADR-12; recon §10.2).
//!
//! These four make whole bug-classes impossible to ship: a tenant-less query (cross-tenant
//! IDOR), a fire-and-forget publish (a lost event / a causality break), a sandbox-bypassing
//! host exec (a privilege escape), and an untagged PII column (an un-erasable / un-mapped
//! subject) are each caught BEFORE they merge.
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
//!   2. the **workspace scan** (`tests/workspace_clean.rs`) runs the four lints over Myelin's
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
//! The REMAINING EIGHT lints (completing the twelve) land in the SAME crate in **P-S11 →
//! P-018** (`no-cross-db`, `forward-only-migration`, `no-cross-sync-cycle`, `residency-pin`,
//! `control-plane-pii-free`, `search-requires-acl-filter`, `no-llm-in-platform`,
//! `flow-determinism`). The `no-cross-sync-cycle` rule already has a build-layer twin (the
//! `crate-graph-acyclic` test in `myelin-substrate`); P-S11 ships the source-scanning form.

pub mod engine;
pub mod lints;

pub use engine::{Lint, LintId, Violation};
pub use lints::{
    no_host_exec, no_raw_publish, no_untagged_personal_data, tenant_predicate, LOAD_BEARING_FOUR,
};

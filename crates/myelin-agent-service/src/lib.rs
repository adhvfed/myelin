//! # `myelin-agent-service` — the Agent-Fabric data model (run / tool_def / proposed_effect /
//! hitl_gate / trace), `(tenant, region)`-first + RLS + the tenant-predicate lint (AG-P2 / P-131)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/agent-fabric.md`
//! §4 (the data model — the five tables, all `(tenant, region)`-first, RLS-enforced, no
//! cross-tenant query path, residency-pinned, per-tenant envelope-encrypted, `PersonalDataHolder`;
//! the exact field lists §4.1..§4.5). Carried forward from Phase-3 §4.
//!
//! **Contract-index:** rows 12.1 (the Tenancy `(tenant, region)` partition key from the verified
//! token), 11.3/11.4 (per-subject DEK envelope encryption), 1.5 (forward-only online migrations),
//! 1.6 (the `tenant-predicate` + `forward-only-migration` lints — the loud, committed ratchet
//! gates). Implemented to the frozen shapes.
//!
//! **VISION §3** (GDPR-safe & EU-sovereign by construction: residency + RLS are *architectural*,
//! not a runtime check that can be forgotten). **EI-01 §5** (the committed ratchet — the
//! tenant-predicate + forward-only-migration lints are loud gates), **§3** (a property does not
//! exist until a test forces it — the cross-tenant-read denial test, `tests/integration_rls.rs`).
//!
//! ## This crate is the IMPLEMENTATION crate, distinct from the glue crate `myelin-agent`
//! `myelin-agent` (AG-P1 → P-130) is the **glue crate** — the frozen six-trait contract surface +
//! the `ToolDef` / `EffectKind` / `EffectResult` value types, NO engine logic. **This** crate is the
//! Fabric's **service / implementation** crate (architecture §4 names them distinct). At AG-P2 it
//! ships the **data model**: the five forward-only `(tenant, region)`-first RLS migrations + the
//! schema row tag-carriers. The runtime that drives these tables lands later: the SKELETON runtime
//! (AG-P4 → P-216), `MockAgentRuntime` (AG-P5 → P-217), the plan-then-apply `EffectApi` pipeline
//! (AG-P6 → P-218).
//!
//! ## The five tables (architecture §4.1..§4.5) — all `(tenant, region)`-first, RLS-enforced
//! - **`run`** (§4.1) — the unit of agent execution, a durable-workflow instance (ADR-09). A run may
//!   pause for *days* on a HITL gate holding no thread.
//! - **`tool_def`** (§4.2) — the one permissioned registry. The `requires_approval` COLUMN exists
//!   here; its per-subsystem **seed defaults** land in AG-P8 (→ P-220), not here.
//! - **`proposed_effect`** (§4.3) — the plan-then-apply audit row: every proposed effect recorded
//!   whether applied, gated, or denied.
//! - **`hitl_gate`** (§4.4) — the approval state, a durable-workflow wait surfaced as a chat card.
//! - **`trace`** (§4.5) — the content-addressed execution-trace pointer (`run.trace_ref` is its
//!   `ArtifactRef`). The trace is a `PersonalDataHolder`; the holder body lands with Knowledge in M3
//!   (AG-P19 → P-268). Here the column + the residency pin exist.
//!
//! ## The `(tenant, region)`-first + RLS construction (the IDOR floor — storage §1.1)
//! Every one of the five tables leads with `(tenant_id, region)` and is made RLS-ready by the
//! `myelin_make_tenant_scoped(table)` convention the dev/prod Postgres init installs
//! (`scripts/pg-init/00-rls-conventions.sql`): `ENABLE` + `FORCE ROW LEVEL SECURITY` + the standard
//! `(tenant_id, region)` isolation policy keyed on `current_setting('myelin.tenant_id')` /
//! `current_setting('myelin.region')`. The app role is `NOSUPERUSER NOBYPASSRLS`, so a session set
//! to tenant A reads **only** tenant A's rows — **0 cross-tenant rows readable**, enforced in
//! Postgres, not just app code. The migrations run through the storage forward-only **online**
//! runner ([`myelin_storage::migration::OnlineMigrationRunner`]) so they are forward-only by
//! construction (a `DROP` / a blocking `ALTER` on a hot table / a contract-before-backfill is
//! refused). See [`migrations`].
//!
//! ## The lints this crate is bound by (contract 1.6 — PERMANENT ratchet gates)
//! - **`tenant-predicate`** — every query against a tenant-owned table threads the `(tenant,
//!   region)` predicate; a tenant-less query is a cross-tenant IDOR and is rejected. The agent-shaped
//!   red+green fixtures live in `crates/myelin-lints/tests/fixtures/tenant_predicate.agent.*` and are
//!   exercised by `crates/myelin-lints/tests/agent_lints.rs`.
//! - **`forward-only-migration`** — no rollback/down migration; no in-place rewrite; no blocking
//!   `ALTER` on a hot table; the online expand→backfill→contract shape only. Agent-shaped red+green
//!   fixtures live alongside the above (`forward_only_migration.agent.*`).
//!
//! ## Floors named (state cross-references; VISION §3)
//! - **The `PersonalDataHolder` REGISTRATION seam lands in AG-P3 (→ P-132).** Here the five tables
//!   exist and carry their `#[personal_data(...)]` classification tags; the holder *registration*
//!   (so the harness auto-registers the Fabric's holders on boot) is the very next prompt.
//! - **The `PersonalDataHolder` BODIES (locate / export / erase) land in AG-P23 (→ P-1371-band).**
//!   The schema is complete and the crypto-shred lever (per-subject DEK) exists by tag here; the full
//!   DSR fan-out across all Fabric holders (run table, trace, agent memory) is the M5 follow-on
//!   (drill AG-D10 — erasure reaches the trace + memory).
//! - **The trace HOLDER body lands with Knowledge (AG-P19 → P-268, KN-D11/KN-D12).** Here the
//!   `trace` table + the `run.trace_ref` `ArtifactRef` column + the residency pin exist; the
//!   content-addressed write of the trace document into Knowledge is that follow-on.
//! - **The concrete DDL execution against a live Postgres connection** is the storage driver's
//!   (P-S12); here the [`migrations::runner`] *validates ordering + admits the online shape* and the
//!   `integration` test proves the RLS policy denies a cross-tenant read against the LIVE dev stack.
//!   The validation logic does not change shape when the driver lands.

pub mod migrations;
pub mod schema;

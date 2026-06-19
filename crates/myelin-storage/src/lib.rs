//! # `myelin-storage` — the OLTP tier client (harness pool + `(tenant, region)` RLS guard)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §1.1 (the non-negotiables
//! every tier inherits — tenant is the first column / partition key, sourced from the
//! verified token, NEVER the URL path; no cross-tenant query path), §3.1 (Tier 1 OLTP:
//! Postgres-class, one DB per service, the `(tenant, region)`-first RLS tenant-scoping
//! guard = the IDOR floor + the `tenant-predicate` lint target, bounded pools + statement
//! timeouts), §2 (the store map, T1 row).
//!
//! **Contract-index cluster:** 11 — Storage (row 11.1 *OLTP tier client — harness pool +
//! RLS half*) + consumed rows 12.1 (`(tenant, region)` partition key), 1.1/1.4/1.8
//! (harness + holder auto-registration + telemetry). This prompt is **P-ST-01 → global
//! P-007**.
//!
//! ## What this crate is (and is NOT) — the implementation-crate note
//! The Storage by-system prompt file (§47-52) is explicit: *Storage's runtime code lands
//! in a new workspace crate `myelin-storage` — the tier clients, the KMS adapters, the
//! `BlobStore` impls, the backup/restore machinery.* This crate is that **storage
//! substrate**: the harness-level seam `serve(AppSpec)` wires every subsystem's OLTP pool
//! through (NOT a hand-rolled connection). It is the home for the `(tenant, region)` RLS
//! guard, the bounded pool, and (in later prompts) the KMS hierarchy and BlobStore impls.
//!
//! ## DEVIATION FROM THE FROZEN CRATE-DAG SHAPE (EI-01 §1 — code wins, write it down)
//! The substrate architecture (00 §2.8) says there is **deliberately no shared "storage
//! API" crate spanning subsystems** (each service owns its schema; the boundary is the
//! `no-cross-db` lint, not a shared data-access crate), and §2.9 lists the crate DAG as
//! ten crates with **no `myelin-storage` node**. BUT the Storage by-system prompt
//! mandates a `myelin-storage` crate for Storage's *runtime* code (the tier clients / KMS
//! / BlobStore impls), and 11.1 (the OLTP tier client) is genuinely a *shared mechanism*
//! every subsystem opens its pool THROUGH (`serve(AppSpec)` wires it) — the opposite of a
//! per-subsystem schema crate.
//!
//! Resolution (the minimal reconciliation): `myelin-storage` is the **storage SUBSTRATE**,
//! not a cross-subsystem data-access crate. It carries the harness-wired *mechanism* (the
//! pool, the RLS guard, the holder hook), exactly the thin, visible query layer §2.8 says
//! the harness provides ("a query builder + typed rows, not an ORM"). The `no-cross-db`
//! rule is preserved: a subsystem still owns its own schema and
//! opens its OWN pool through this seam; this crate exposes the GUARD, not another
//! subsystem's tables. In the crate DAG it sits below `-gdpr`/`-client` and ABOVE
//! `-substrate` (the harness depends on the tier client it wires) — extending the §2.9
//! root-last order with one node. The `crate_graph` model in `myelin-substrate` is updated
//! to 11 crates accordingly. Flagged in the P-007 report; if the architecture is later
//! re-frozen to forbid this node, the guard moves into `myelin-substrate` unchanged.
//!
//! ## The load-bearing fact this crate sequences around (storage.md §1.1, EI-01 §2)
//! **Cross-tenant IDOR is the stop-the-bleeding, order-by-non-negotiability floor.** The
//! `(tenant, region)` predicate on every tenant-table query is sourced from the **verified
//! token**, never the URL path — a read whose token-tenant ≠ path-tenant resolves to the
//! **token-tenant**, with `path_derived_tenant_count == 0` (the §1.1 IDOR floor; the
//! [`SignalName::CrossTenantCount`](myelin_harness) survival signal the IDOR drill asserts
//! `== 0`). The [`rls`] module is the mandatory-core whose derivation is mutation-tested
//! (≥ 80% floor; see the module docs + the P-007 report).
//!
//! ## Floors named (stubbed / deferred + the filling prompt)
//! - **Per-tenant ENVELOPE ENCRYPTION of columns is NOT yet wired.** The KMS hierarchy
//!   lands in M1, so on THIS floor columns are **plaintext-at-rest**. The M1 prompt
//!   **P-ST-08** (global P-095) closes this gap; **no real tenant data is written before
//!   then** (the M1 STOR-D1 restore-verify gate enforces it). This is the plaintext-at-rest
//!   floor the prompt requires recorded in writing — recorded HERE.
//! - **The outbox CO-LOCATION** (the outbox table living in this OLTP DB + the
//!   same-transaction co-commit) — the SIBLING prompt **P-ST-02** (global P-016) — is now
//!   IMPLEMENTED in [`coloc`]: [`ColocatedOltp`] owns the outbox in the same service DB
//!   (its migration set carries [`coloc::COLOCATED_OUTBOX_MIGRATION`]) and [`ColocatedTx`]
//!   co-commits a domain-state write and the outbox insert in one transaction (both commit /
//!   both roll back). The per-aggregate `seq` it establishes is the §7.3 cross-seam cursor
//!   restore consumes (forward dependency **P-ST-14**, global P-100). The outbox *mechanism*
//!   (table DDL + `OutboxTx::emit` + the relay) is reused from `myelin-events` (P-008/P-012/
//!   P-013), never re-defined — this prompt adds only the OLTP co-location binding.
//! - **A real Postgres pool.** The substrate's `serve(AppSpec)` DB-pool body is itself a
//!   `todo!()` floor (P-S12/P-S15). This crate's [`OltpPool`] is therefore a
//!   backend-agnostic, in-memory-testable pool MODEL (bounded permits + statement-timeout
//!   config + per-tenant in-flight caps) over the SAME `AppSpec` config the harness
//!   validates; the concrete `tokio-postgres`/`sqlx` connection lands when `serve`'s pool
//!   body does (P-S12). The RLS guard + the bounded-pool semantics + the holder hook are
//!   complete and testable now and do not change shape when the driver lands.
//! - **`PersonalDataHolder` BODIES** (locate/export/rectify/restrict/erase) are the GDPR
//!   M1 deliverable; here only the **registration hook fires** (1.4) — see [`holder`].

pub mod coloc;
pub mod holder;
pub mod oltp;
pub mod rls;

pub use coloc::{ColocError, ColocatedOltp, ColocatedTx, COLOCATED_OUTBOX_MIGRATION};
pub use holder::{register_holder, OltpHolderRegistration, OltpStoreHolder};
pub use oltp::{OltpConfig, OltpError, OltpPool, PermitGuard};
pub use rls::{RlsError, TenantQuery, TenantScope, TenantTable};

//! # `myelin-gdpr-service` — the GDPR/Audit subsystem service (P-GA-19 → P-062)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` §6.1–§6.2 (the
//! tamper-evident audit log: ONE append-only log of every human AND agent action, a
//! per-tenant hash-chain whose entries are also Merkle-tree leaves, written via the outbox
//! only, minimised by design). The entry/STH schema is `03-shared-systems-architecture/
//! gdpr-and-audit.md` §6.2 (unchanged). Doctrine:
//! `external-insights/01-process-and-quality-doctrine.md` §5 (*written via the outbox only —
//! coverage is a property of the bus, not a per-service good intention*).
//!
//! **Contract-index:** row **10.6** — the audit-log CONSTRUCTION half (the outbox consumer +
//! the per-tenant hash-chain + the Merkle leaves + the minimisation). The PROOFS half
//! (inclusion / consistency / signed-tree-head + the independent-witness anchoring + the
//! DSR-receipt seal + the H16 carve-out body) is **P-GA-20 → P-119** (contract 10.6 row stays
//! pointed at it for the proofs; this prompt ships the construction the proofs will prove over).
//!
//! ## What this crate ships (P-GA-19, the construction half of 10.6)
//! The tamper-evident audit log CORE, in [`audit`]:
//! 1. **The audit consumer** ([`audit::AuditConsumer`]) — an infra subscription on the M0 outbox
//!    (an [`myelin_events::EventHandler`]): every action-bearing event (human + agent — agents are
//!    audited identically, EI-02 §2) is appended to the log. **No service writes the log directly**
//!    — the ONLY way to write the log is to deliver an event through this consumer (coverage is a
//!    bus property; the [`audit::AuditLog::append`] API is crate-private to this module so there is
//!    no direct-write path outside the consumer). The architecture test
//!    [`audit::tests::no_service_writes_the_audit_log_except_the_outbox_consumer`] asserts it.
//! 2. **The per-tenant hash-chain** ([`audit::AuditLog`]) whose entries are also **Merkle-tree
//!    leaves** ([`audit::AuditEntry::leaf_hash`] / [`audit::AuditEntry::prev_hash`]): a retroactive
//!    edit/deletion breaks the chain from that point forward (Haber–Stornetta), and each entry is
//!    `O(log n)`-provable as a Merkle leaf (RFC 6962 / Crosby–Wallach). Deliberately **NOT a
//!    blockchain** (no global byzantine consensus; a residency problem if replicated off-cell —
//!    gdpr §6.1, the written deviation).
//! 3. **Minimised by design** ([`audit::Minimised`]): `actor` / `on_behalf_of` / `subject` are
//!    pseudonymous IDs / [`myelin_tenancy::ArtifactRef`]s, **never payloads**; `actor` uses the
//!    frozen pseudonym grammar `<pseudonym>@<tenant>.noreply` (contract 4.8). The minimised form is
//!    constructed FROM the verified [`myelin_identity::Principal`]'s opaque, PII-free
//!    `principal_id` — a name/email can never reach an entry because the entry holds no field for it.
//! 4. **Causality-carried**: `correlation_id` / `causation_id` are copied off the
//!    [`myelin_events::EventEnvelope`] verbatim — the audit log IS the "why did this happen" walk
//!    (one mechanism for audit + provenance + the loop guard, EI-02 §6).
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - The **CT-style proofs** (`inclusion_proof` / `consistency_proof` / `signed_tree_head`), the
//!   **independent-witness anchoring** (RFC-3161 TSA / a different cell's notary), the
//!   **DSR-receipt seal**, and the **H16 audit-carve-out body** → **P-GA-20 / P-119** (the
//!   construction here is what those proofs run over; this crate stores the full chain + the
//!   incremental Merkle root the STH will sign).
//! - The **`git.history_rewrite` audited op** (gdpr §6.6, GA-10) → **M5 P-GA-35**.
//! - The **durable Postgres `audit_entry` / `audit_sth` tables** (the §6.2 DDL) + running the
//!   consumer inside `serve(AppSpec)` against the OLTP pool: the seam shape (the [`audit::AuditEntry`]
//!   row, the `(tenant, seq)` PK, the append-in-the-same-tx-as-the-dedup-mark atomicity) does NOT
//!   change; the live DB write lands when the OLTP client is wired here (the same floor every M0
//!   in-memory store carries — P-007 / P-S12). On this floor the chain is an in-memory model with
//!   byte-for-byte the §6.2 semantics.
//! - The **`audit_append_lag` telemetry on the metrics-health port**: the SLO signal NAME + UNIT
//!   are anchored here ([`audit::AUDIT_APPEND_LAG`]) and the consumer exposes the live measurement
//!   ([`audit::AuditConsumer::append_lag`]); wiring it onto the running service's metrics-health
//!   surface is the `serve(AppSpec)`-boot follow-on (P-119 rides the same surface).
//!
//! ## Mutation floor (P-GA-19 TESTS — the hash-chain-append + Merkle-leaf paths are
//! mandatory-core). `cargo mutants --package myelin-gdpr-service` (2026-06-19): 64 mutants, 46
//! caught, 6 timeouts (the `merkle_root` reduction loop — not survivors), 7 unviable, 5 missed.
//! Every BEHAVIORAL mutant on the mandatory-core paths — `AuditLog::append`,
//! `ActionRecord::leaf_preimage` (the Merkle leaf), the chain-link computation,
//! `Outcome::as_str` (part of the leaf preimage), and `verify_entries` (the integrity core) — is
//! CAUGHT. The 5 residuals are documented non-core: (1) `AuditLog::verify_chain -> true` is the
//! thin public wrapper that delegates to `verify_entries` (whose logic IS mutation-killed by the
//! tamper / re-order / drop tests) — a corrupt in-store chain is unreachable through the public
//! API, so the wrapper's branch cannot be exercised false from outside; (2) a `Vec::with_capacity`
//! hint in `merkle_root` (an equivalent mutant — capacity is a perf hint, not behavior); and
//! (3)–(5) the `audit_append_lag` accessor + lag-bump and the empty `subjects()` whitelist — both
//! on NAMED FLOOR surfaces whose live behavior lands later (the metrics-health wiring → P-119; the
//! per-subsystem subject roster → P-GA-26). Stated, not hidden (EI-01 §3).
//!
//! ## DAG position (a named §2.9 extension — like `myelin-identity-service`)
//! This is a SERVICE crate, the GDPR/Audit subsystem's bootable home. It is a leaf consumer
//! ABOVE `myelin-events` / `myelin-identity` / `myelin-tenancy`: it depends on the frozen bus +
//! identity + tenancy surfaces and NOTHING in the production library DAG depends back on it
//! (`crate_graph.rs`'s `substrate_is_root()` is preserved — a service is the graph's terminal
//! consumer, not a node in the eleven-crate library graph). The DSR orchestrator + the rest of the
//! GDPR-owned holders (contract cluster 10) come to live here across M1 (P-GA-06/-11); P-GA-19
//! seeded the crate with the audit-log core.
//!
//! ## P-GA-05 (→ P-105) — the `PersonalDataHolder` trait bodies + the GDPR-owned holders
//! [`holders`] ships the **bodies** of the frozen contract-10.1 `PersonalDataHolder` trait, and the
//! GDPR-owned holder impls: **H18** ([`holders::GdprOwnStoreHolder`], GDPR's own G1–G7 registers —
//! `erase` crypto-shreds the per-tenant/-subject DEK, 0 recoverable after) and **H16** the audit
//! carve-out ([`holders::AuditCarveOutHolder`] — `erase` retains the minimised pseudonym record,
//! NEVER rewrites the chain, expires via audit-key crypto-shred). The crypto-shred MECHANISM is
//! reached through the [`holders::CryptoShredKms`] **seam** (Storage owns the `KmsEngine`; the
//! no-cross-store-read law forbids a `myelin-storage` import — asserted by
//! `holders::tests::gdpr_service_has_no_cross_store_read_import`). The upstream-store orchestration
//! (H6/H8/H9/H10/H14/H15 + the canonical erase order) is **P-GA-06 → P-106**; GA-D1 (0 holders
//! missed, the whole map) is the **M5 gate P-GA-32 → P-505**.
//!
//! ## P-GA-09 (→ P-109) — the data-map / RoPA generator (contract 10.3)
//! [`datamap`] ships the **generated data map** (`data_map() → Inventory`) + the **RoPA projection**
//! (`ropa(tenant) → ProcessingActivities`). The generator WALKS the compile-time `#[personal_data]`
//! registry (from `myelin-gdpr`'s classify-derive, P-107) + the runtime auto-registered holder set
//! (the [`myelin_substrate::HolderRegistration`]s, P-S15 / P-GA-04, classified into the exhaustive
//! H1–H18 [`myelin_substrate::Holder`]) and GENERATES the machine-readable inventory: every PII
//! field, its owning holder, the five tags, the subject_locator, the residency region, the DPIA
//! marker. *The map, not a hand-written list, drives erasure* — GA-D1's "0 holders missed" is a
//! property of the generated map ([`datamap::Inventory::coverage_gaps`] surfaces a holder in the
//! registry but absent from the map; the entry count equals [`datamap::tagged_field_count`]).
//! **Floors named:** the CI data-map **DIFF GATE** (commit the inventory; a build that changes it
//! fails CI with the diff surfaced until a DPO reviews) → **P-GA-10 → P-110** (this crate ships the
//! generation + the deterministic [`datamap::Inventory::fingerprint`] the diff compares); the
//! per-store content **completeness** floor → **M5 P-GA-32 → P-505**; the **RoPA legal text** is
//! **`[OPEN — LEGAL]`** (the GENERATION ships here; the DPO ratifies the characterisation).

pub mod audit;
pub mod datamap;
pub mod holders;
pub mod orchestration;

pub use audit::{
    AuditConsumer, AuditEntry, AuditLog, Minimised, Outcome, AUDIT_APPEND_LAG,
};
pub use datamap::{
    data_map, ropa, ropa_for_tenant, tagged_field_count, HolderSchema, Inventory, InventoryEntry,
    ProcessingActivities, ProcessingActivity, DATA_MAP_ENTRY_COUNT, DATA_MAP_HOLDER_COUNT,
};
pub use holders::{
    gdpr_owned_holder_ids, AuditCarveOutHolder, CryptoShredKms, GdprOwnStoreHolder,
    InMemoryShredKms, ShredKeyClass, ShredKeyHandle, AUDIT_CARVE_OUT_STORE, GDPR_OWN_STORE,
};
pub use orchestration::{
    canonical_phase_of, holder_ids, CanonicalErasePhase, EraseChecklist, HolderReceipt,
    RegisteredHolder, SeamHolder, UpstreamHolderOrchestrator, CRYPTO_SHRED_LAG,
    ERASURE_FANOUT_COVERAGE,
};

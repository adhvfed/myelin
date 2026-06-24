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
//! ## P-GA-20 (→ P-119) — the audit CT-style proofs + STH + witness + DSR-receipt seal + H16 carve-out
//! [`audit_proofs`] ships the PROOFS half of contract 10.6 OVER the [`audit`] construction: the
//! three CT-style proofs (`signed_tree_head` / `inclusion_proof` / `consistency_proof`, RFC 6962)
//! served by [`audit_proofs::AuditAuthority`]; the **independent-witness anchoring**
//! ([`audit_proofs::Witness`] / [`audit_proofs::NotaryWitness`] — the witness sees ONLY the opaque
//! root, no PII crosses, residency-safe); the **DSR-receipt seal**
//! ([`audit_proofs::AuditAuthority::seal_dsr_certificate`] — a DSR completion certificate is sealed
//! into the per-tenant tree via the SAME outbox-consumer append path, closing the P-GA-12
//! `merkle_inclusion = None` floor); and the **H16 carve-out at the tree level**
//! ([`audit_proofs::AuditAuthority::carve_out_erase`] — a subject erasure RETAINS the minimised
//! entry and NEVER rewrites it, proven by an unchanged STH root + an intact chain). **GA-D3** (a
//! retroactive edit is detected THREE independent ways — the chain break + the consistency-proof
//! failure against the published STH + the witness mismatch) emits its dated green artifact in
//! `tests/ga_d3_audit_tamper.rs` (tamper detected 100%). The auditor-side CDC pair is
//! `tests/cdc_10_6_audit_proofs.rs`. **Floors named:** GA-D3 at CELL scale + the E2E-3 audit leg +
//! the `git.history_rewrite` audited op → **M5 P-GA-35**; the real in-cell KMS signing key
//! (P-ST-06) + a real RFC-3161 TSA witness + the durable `audit_sth` table are the same DB/KMS
//! floor every M0/M1 store carries (P-007 / P-S12) — swapping the [`audit_proofs::SigningKey`] /
//! [`audit_proofs::Witness`] impl is a config swap, not a code change. **Mutation floor (P-GA-20
//! TESTS — the Merkle-proof + consistency-proof verification paths are mandatory-core):** the
//! `cargo mutants` score for [`audit_proofs`] is recorded in the commit body.
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
//! ## P-GA-26 (→ P-153) — eDiscovery export (10.7) + the agent-trace H17 seam (8.8) + the history-rewrite resumable-activity skeleton (10.6)
//! Three deliverables, each REUSING the audit substrate / the holder seam / the durable-activity
//! idiom rather than re-implementing them (EI-01 §7 coherence):
//! - **[`ediscovery`] — the eDiscovery / legal-hold export (contract 10.7).** The
//!   [`ediscovery::EDiscoveryExporter`] is a READ-side authority OVER the existing
//!   [`audit_proofs::AuditAuthority`] (the per-tenant Merkle tree + STH + inclusion proofs,
//!   P-GA-19/P-GA-20) + the existing [`fanout::LegalHoldRegistry`] (the G4 hold gate, P-GA-12). An
//!   `ediscovery_export(scope) → MerkleProvenBundle` (subject/tenant/matter scope) is
//!   **content-addressed** ([`ediscovery::EDiscoveryBundle::bundle_digest`] binds the exact record
//!   set), **inclusion-proof-bearing** (every [`ediscovery::EDiscoveryRecord`] carries its `O(log n)`
//!   proof against the bundle STH — a recipient runs [`verify_inclusion`], "the unaltered record" is
//!   *checked* not *asserted*, EI-01 §3), and **legal-hold-frozen** (the export PLACES a hold so the
//!   records cannot be erased while the bundle is assembled — §5.4). The dual-use of the ONE
//!   tamper-evident substrate (prove-we-erased-it / prove-this-is-the-record) is coherent by
//!   construction — both ride the same per-tenant tree + STH + witness. The CDC pair for 10.7 (a
//!   legal/auditor consuming + verifying the export) is `tests/cdc_10_7_ediscovery_export.rs`.
//! - **[`agent_trace_seam`] — the agent-trace H17 holder seam (8.8), DISTINCT from the audit log.**
//!   The GDPR-orchestration SEAM ([`agent_trace_seam::AGENT_TRACE_HOLDER_ID`] +
//!   [`agent_trace_seam::agent_trace_phase`]) the DSR fan-out registers H17 through, with the
//!   **distinct-from-audit boundary** an architecture test asserts
//!   ([`agent_trace_seam::trace_is_distinct_from_audit`] — trace = erasable crypto-shred; audit = the
//!   retain carve-out; different holder id, different H-number, different mechanism — gdpr §3.2 H17 /
//!   §6.5). The holder BODY is a LOUD named floor ([`agent_trace_seam::AgentTraceHolderSeam`] returns
//!   an error naming **M3 P-GA-27**, never a silent false-green); the live content-addressed trace
//!   `locate`/`export`/`erase` over the Knowledge block model lands in P-GA-27. The id is the SAME
//!   `agent_fabric_trace` name the agent subsystem registers its trace store under (`myelin-agent-
//!   service::holder`, P-131) — ONE name across the seam, reconciled-in-place not duplicated. The CDC
//!   pair for 8.8 is `tests/cdc_8_8_agent_trace_seam.rs`.
//! - **[`history_rewrite`] — the history-rewrite resumable-activity SKELETON (gdpr §6.6 / GA-10).**
//!   A resumable, idempotent [`history_rewrite::HistoryRewriteActivity`] over the ordered
//!   [`history_rewrite::RewritePhase`]s (audit → rewrite → crypto-shred-pack-tier → invalidate-caches)
//!   — the same §4.1-step-4 durable-activity idiom the DSR fan-out + the deadline timer use. A
//!   re-drive after a crash runs ONLY the un-receipted phases + returns the SAME receipts (the
//!   resumability proof). The audit action token [`history_rewrite::HISTORY_REWRITE_ACTION`]
//!   (`git.history_rewrite`) is pinned; the **invalidation fan-out phase is the NAMED M5 floor** (the
//!   trust-tier cache namespaces it fans over do not exist until M5) — a loud deferral, and the
//!   off-platform-clones residual is **named, not pretended-solved** (§6.6). **Floor named:** the
//!   first-class audited op + the invalidation fan-out → **M5 (P-GA-35, GA-10)**.
//! - **Mutation floor (P-GA-26 TESTS — the export-inclusion-proof + the trace-distinct-from-audit +
//!   the resumable-idempotent-activity paths are mandatory-core).** `cargo mutants -p
//!   myelin-gdpr-service` over the three new files (2026-06-20): [`ediscovery`] **23 mutants, 19
//!   caught, 4 unviable, 0 missed**; [`agent_trace_seam`] + [`history_rewrite`] **49 mutants, 24
//!   caught, 24 unviable, 1 missed**. Every BEHAVIORAL mutant on the mandatory-core paths is CAUGHT —
//!   the export's per-record proof attachment + the bundle content-address (a dropped/added/reordered
//!   record fails `verify`), the scope-token + record-proof serialisation, each distinctness conjunct
//!   ([`agent_trace_seam`]'s factored `distinctness_holds` — a same-id / same-H / same-erasability all
//!   collapse distinctness), and the resumable activity's per-phase resume + `skeleton_complete` +
//!   phase tokens. The 1 residual is documented non-core: `trace_is_distinct_from_audit -> true` is
//!   the thin public wrapper that delegates to `distinctness_holds` with the REAL constants (which
//!   ARE distinct) — its boolean output is unobservable-false through the public API (the same
//!   equivalent-wrapper class as `audit::verify_chain -> true`), while its delegation LOGIC is
//!   mutation-killed by `each_distinctness_conjunct_is_load_bearing`. Stated, not hidden (EI-01 §3).
//!   **No
//!   `--features integration` leg owed:** all three compose already-shipped in-memory seams (the audit
//!   tree, the hold registry, the durable-activity model) and touch NO new DB / object-store / cache /
//!   bus contract — the eDiscovery export READS the existing audit log + freezes through the existing
//!   hold gate; the durable `legal_hold`/`audit_entry`/`audit_sth` tables are the same DB floor every
//!   M0/M1 store carries (P-007 / P-S12).
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
//!
//! ## P-GA-10 (→ P-110) — the CI data-map DIFF GATE + the DPIA-route on reclassification (contract 10.3)
//! [`diffgate`] ships the **CI data-map diff gate** — the committed inventory ([`diffgate::CommittedBaseline`])
//! is the DPO-reviewed baseline; a build regenerates the inventory and [`diffgate::check_against_baseline`]
//! compares it: an UNCHANGED map passes ([`diffgate::GateVerdict::Unchanged`]); a CHANGED one (a new PII
//! field, a reclassification, a holder added/removed) FAILS ([`diffgate::GateVerdict::Changed`]) with the
//! structured [`diffgate::DataMapDiff`] surfaced — *until a DPO reviews and re-seals the baseline*. A
//! newly-appeared `SpecialCategory` flow additionally routes into the **DPIA gate** (the
//! [`diffgate::DataMapDiff::dpia_verdicts`] the [`myelin_gdpr::DpiaRouter`] from P-108 drives — *surfaced for
//! a DPO, never auto-decided*). This is GA-D5's data-map-diff face; the companion `no-untagged-personal-data`
//! lint (P-GA-03) is the COMPILE-time half (an untagged PII field never reaches the map). **Floors named:**
//! the map's content **completeness** grows per store as holders ship → **M5 P-GA-32 → P-505** (the gate
//! surfaces each new holder's fields as an additive diff a DPO reviews); the **CI pipeline wiring** (the
//! committed baseline file + the build step that fails the pipeline on a red verdict) lands with the
//! `serve(AppSpec)` boot that assembles the full registered-holder set (the gate LOGIC is complete + tested
//! here — the pipeline invocation is one call to [`diffgate::check_against_baseline`] over the boot's holder
//! set).
//!
//! ## P-GA-11 (→ P-111) — the DSR orchestrator API + the state machine + the posture gate (contract 10.4)
//! [`dsr`] ships the **DSR orchestrator** ([`dsr::DsrOrchestrator`]) — the three API entry points
//! (`dsr_submit(kind, subject, scope, posture) → dsr_id`, `dsr_status(dsr_id) → {state, deadline,
//! checklist}`, `dsr_certificate(dsr_id) → MerkleProvenBundle`), the **total + ordered state machine**
//! ([`dsr::DsrState`]: `received → validated → fanned-out → {awaiting-holders} → verified → completed`, with
//! the `awaiting-holders`-cannot-be-skipped property the single transition guard
//! [`dsr::DsrState::can_transition_to`] enforces), and the **controller/processor posture gate** (§1 — a
//! Myelin-initiated erase of *tenant content* is REFUSED, [`dsr::DsrState::Refused`], unless tenant-instructed
//! or a `EraseScope::Tenant` offboarding). The deadline is set COARSE on submit (`now + 1 month` via an
//! injectable [`myelin_substrate::Clock`]). The orchestrator RESOLVES a read-only per-holder checklist FROM
//! the generated [`datamap::Inventory`] (the map, not a hand-written list, drives the scope — §4.1 step 2).
//! **Floors named:** the per-holder checklist DRIVE + the resumable fan-out + the verifiable receipts + the
//! legal-hold gate → **P-GA-12 → P-112**; tenant-operability (Art. 28 + offboarding + restrict/rectify/
//! portability) → **P-GA-13 → P-113**; the durable deadline timer (the `myelin-flow` wheel) → **M2 P-GA-21 →
//! P-148**; the Merkle SEAL of the certificate receipts into the per-tenant audit tree → **P-GA-20 → P-119**
//! (the [`dsr::MerkleProvenBundle::merkle_inclusion`] field is `None` until then); the durable Postgres
//! `dsr_request`/`dsr_receipt` (G1/G2) tables → the same DB floor every M0 in-memory store carries (P-007).
//!
//! ## Mutation floor (P-GA-11 TESTS — the state-machine transitions + the posture gate are mandatory-core).
//! See the module note in [`dsr`]; the [`dsr::DsrState::can_transition_to`] guard, the
//! [`dsr::DsrOrchestrator::posture_gate_refuses`] predicate, and the `now + 1 month` deadline are the
//! behavioral core every mutation must be caught on (the `cargo mutants` score for this module is recorded in
//! the commit body — EI-01 §3, stated not hidden).
//!
//! ## P-GA-12 (→ P-112) — the data-map-driven checklist + the resumable fan-out + receipts + the legal-hold gate (contract 10.4)
//! [`fanout`] ships the **DSR fan-out driver** ([`fanout::FanOutDriver`]) — it DRIVES the §4.1
//! algorithm by tying the DSR spine ([`dsr::DsrOrchestrator`], P-GA-11) + the canonical-order
//! resumable holder fan-out ([`orchestration::UpstreamHolderOrchestrator`], P-GA-06) + the NEW
//! **legal-hold gate** ([`fanout::LegalHoldRegistry`], G4) together: (1) it resolves the per-holder
//! checklist FROM the data map (the map, not a hand-written list, drives the scope — §4.1 step 2);
//! (2) it applies the **legal-hold gate** (§4.1 step 3 — an erase under an active hold is DEFERRED
//! *partially*, fail-safe-to-suspend; a read right is never suspended); (3) it **fans the erase out**
//! through the holder contract in the canonical erase order, idempotently + resumably (the durable
//! [`orchestration::EraseChecklist`] IS the state — a worker kill re-drives only un-receipted
//! holders, 0 double-erase); (4) it collects + verifies the receipts and constructs the
//! **verifiable content-addressed DSR completion receipt** ([`fanout::DsrCompletionReceipt`],
//! §4.2 — `request_id ∥ holder ∥ scope ∥ outcome ∥ key_epoch_destroyed? ∥ timestamp`); and (5) it
//! completes the DSR state machine. It REUSES both orchestrators wholesale (EI-01 §7 coherence — no
//! re-definition of the state machine, the checklist, or the fan-out). **Floors named:** the Merkle
//! SEAL of the completion receipt into the per-tenant audit tree → **P-GA-20 → P-119** (this prompt
//! CONSTRUCTS the content-addressed receipt; [`dsr::MerkleProvenBundle::merkle_inclusion`] stays
//! `None`); the end-to-end erasure PROOF (0 recoverable incl. backups + worker-kill resumability
//! across a restart) → **P-GA-14 → P-114** (reuses this driver); the multi-cell `member_cells`
//! iteration → **M5 P-GA-33**; the retention-expiry suspend under a hold (the tightest-policy-wins
//! retention engine) → **M2 P-GA-22 → P-149** (the GATE is wired here); the durable Postgres
//! `legal_hold` (G4) / `dsr_receipt` (G2) tables → the same DB floor every M0 in-memory store
//! carries (P-007). **Mutation floor (P-GA-12 TESTS):** the [`fanout::LegalHoldRegistry::verdict`]
//! gate, the [`fanout::FanOutDriver::drive`] sequence, and the [`fanout::DsrCompletionReceipt`]
//! content-address are mandatory-core; the `cargo mutants` score is in the commit body.
//!
//! ## P-GA-13 (→ P-113) — DSR tenant-operability: Art. 28 + offboarding + restrict/rectify/portability (contract 10.4)
//! [`tenant_ops`] ships the **DSR tenant-operability surface** ([`tenant_ops::TenantDsrSurface`]) —
//! the orchestrator EXPOSED to **tenants** for *their own* data subjects (§4.4): (1) **Art. 28**
//! tenant-facing DSR ([`tenant_ops::TenantDsrSurface::submit_for_my_subject`]) with the **Art-28
//! scoping guard** (a tenant may only act for a subject in ITS OWN tenant — a cross-tenant request is
//! REFUSED, [`tenant_ops::TenantDsrError::CrossTenantSubject`], the cross-tenant-IDOR/SUB-D7 GDPR
//! face), encoded `Initiator::TenantInstructed` + `Posture::Processor` (the customer org is the
//! controller — the posture gate ADMITS even a processor-posture erase the tenant instructed); (2)
//! **tenant offboarding** ([`tenant_ops::TenantDsrSurface::offboard_tenant`]) = `erase(EraseScope::
//! Tenant)` fanned over the holder list (the tenant KEK destroyed ⇒ every DEK unwrappable ⇒ the whole
//! tenant, backups included, unrecoverable), sealing an [`tenant_ops::OffboardingCertificate`]; (3)
//! the **restrict / rectify / portability** entry points routed through the orchestrator
//! ([`tenant_ops::TenantDsrSurface::restrict_subject`] / `rectify_subject` /
//! `portability_for_subject`). It REUSES the DSR spine (P-GA-11) + the fan-out driver + the
//! legal-hold gate (P-GA-12) wholesale (EI-01 §7 coherence — no second state machine, posture gate,
//! or fan-out). **Floors named:** single-cell offboarding ships here; the **multi-cell `member_cells`
//! iteration** over the cross-cell PII-free bridge → **M5 P-GA-33** (GA-D8); the full
//! **`restrict`-honoured-into-derived-stores** proof → **M2 P-GA-25 → P-152**; the
//! **reindex-from-source rectification** derivative fan-out → **M2 P-GA-24 → P-151**; the durable
//! Postgres `dsr_request` table + the live KMS binding for the tenant-KEK shred → the same DB/KMS
//! floor every M0/M1 in-memory store carries (P-007 / the Storage KMS hierarchy). **Mutation floor
//! (P-GA-13 TESTS):** the Art-28 cross-tenant guard, the `EraseScope::Tenant` offboarding fan-out,
//! and the restrict/rectify/portability routing are mandatory-core; the `cargo mutants` score is in
//! the commit body.
//!
//! ## P-GA-24 (→ P-151) — the per-derivative erasure fan-out: Search purge+reindex (incl. embeddings) + Refs tombstone + reindex-from-source rectification (contract 6.4/5.8/2.6 wired; 10.1 orchestration leg)
//! [`derivative_erasure`] ships the **per-derivative erasure fan-out** over the M2 derived stores —
//! the orchestration leg of 10.1 that wires Search/Refs/Notif as the orchestrator's per-holder erase
//! calls, with their derivative-SPECIFIC `erase` semantics (each a REAL purge, never hide): **Search
//! (H7)** [`derivative_erasure::SearchIndexHolder`] — *purge + reindex incl. embeddings* (purged-not-
//! hidden; a re-identification probe [`derivative_erasure::SearchIndexModel::reidentify_hits`] returns
//! **0** after erase — GA-D2 / SRCH-D4); **Refs (H12)** [`derivative_erasure::RefsGraphHolder`] —
//! *tombstone* (0 recoverable, [`derivative_erasure::RefsGraphModel::resolve`] returns a
//! [`derivative_erasure::RefsResolve::Tombstone`], **never a 500** — REF-D5); **Notif (H13)**
//! [`derivative_erasure::NotifHistoryHolder`] — *humanise mentions to `[erased user]`*
//! ([`derivative_erasure::ERASED_USER`] — NOTIF-D6). Plus the **reindex-from-source rectification**
//! ([`derivative_erasure::DerivativeErasureDriver::rectify_via_reindex_from_source`]) — Art. 16's
//! derivative-correction half: the derived stores **rebuild from the corrected source** (drift = 0),
//! NEVER patched-in-place (there is no patch entry point — the structural foreclosure of patch-and-
//! drift, §4.4 / EI-04 §5). It REUSES the [`orchestration::RegisteredHolder`] seam +
//! [`orchestration::CanonicalErasePhase`] order wholesale (the derivative holders register at their
//! [`derivative_erasure::derivative_phase_of`] phases alongside the upstream holders — EI-01 §7
//! coherence; no second orchestrator). The green artifact is the **embedding-purge receipt**
//! ([`derivative_erasure::DerivativeEraseReceipt::embeddings_purged`]). **GA-D2** (the subject's docs
//! AND embeddings purged+reindexed out, 0 embedding re-identification) + **REF-D5** (refs tombstone,
//! 0 recoverable, no resolve-500) + **NOTIF-D6** (inbox humanises to `[erased user]`) emit their dated
//! green artifacts in `tests/ga_d2_derivative_erasure.rs`; the CDC pairs for 6.4/5.8/2.6 are in
//! `tests/cdc_6_4_5_8_2_6_derivative_erasure.rs`. **Floors named:** the `restrict` suppression INTO
//! these same derived stores (GA-D7) → **M2 P-GA-25 → P-152** (this prompt ships the per-derivative
//! ERASE + RECTIFY; the restriction-honoured-into-derived proof rides this fan-out); the **agent-trace
//! H17 seam** → **M2 P-GA-26 → P-153**. The live Search/Refs/Notif `erase` bindings behind the
//! [`myelin_gdpr::PersonalDataHolder`] seam are a config swap at boot (the in-memory models here have
//! byte-for-byte the GA-D2/REF-D5/NOTIF-D6 post-conditions); this module touches **NO new DB / object-
//! store / cache / bus contract — no `--features integration` leg owed**. **Mutation floor (P-GA-24
//! TESTS — the purge-not-hide [embeddings] + the reindex-from-source paths are mandatory-core):**
//! [`derivative_erasure::SearchIndexModel::erase`] (the embedding compaction), the Refs tombstone-not-
//! 500 branch, the Notif `[erased user]` humanise branch, and `rectify_via_reindex_from_source` (the
//! rebuild-never-patch path) are the behavioral core; the `cargo mutants` score is in the commit body.
//!
//! ## P-GA-25 (→ P-152) — `restrict` suppression into the derived stores (Search/Refs/Notif/Agents/OLAP) — GA-D7 (contract 11.6 + the 10.1 derived-store faces)
//! [`restrict_fanout`] FILLS the floor P-GA-17 named: the **`restrict` suppression FLAG** — the ONE
//! [`structural_floor::RestrictRegistry`] shipped in P-117 — is now HONOURED by the **five M2 derived
//! stores**, the full GA-D7 proof. P-117 proved the M1 holders honour the flag; THIS proves the
//! derived stores do — **0 processing of a restricted subject** across Search / Refs / Notif / Agents
//! / OLAP, reversible. It REUSES the [`structural_floor::RestrictRegistry`] WHOLESALE (there is
//! exactly ONE suppression flag per subject — every derived store + M1 store reads the SAME registry,
//! the §4.4 "every holder honours" property; no second flag, no parallel mechanism — EI-01 §7
//! coherence). The genuinely-new piece is the **per-derivative-store PROCESSING op that HONOURS the
//! flag** ([`restrict_fanout::DerivedStore::process`]) — a derived store's chokepoint differs from an
//! M1 store's source-content op: **Search** *no indexing* / **Refs** *no edge projection* / **Notif**
//! *no notification* / **Agents** *no agent-use* / **OLAP** *no analytics* (contract 11.6, GA-9, the
//! §8 restriction-flag-into-OLAP propagation). While restricted, each reads
//! [`restrict_fanout::DerivedProcessed::Suppressed`] while RETAINING the derived row (suppression ≠
//! delete — reversible). The [`restrict_fanout::RestrictFanOutDriver`] fans `restrict(subject, on)`
//! over the five through the [`myelin_gdpr::PersonalDataHolder`] seam (never reaching into a store —
//! the no-cross-store-read law) and reads back the verdicts; the green artifact is
//! [`restrict_fanout::RestrictFanOutOutcome::processed_count`] = **0**. **GA-D7** (restrict → 0
//! processing across all five derived stores, storage retained, reversible) emits its dated green
//! artifact in `tests/ga_d7_derived_restrict.rs`; the CDC pair for 11.6 (the OLAP consumer honouring
//! `restrict`) + the Search/Refs/Notif/Agent restriction faces is in
//! `tests/cdc_11_6_derived_restrict.rs`. **Floors named:** the **multi-cell restriction** (the flag
//! fanned across `member_cells` over the cross-cell PII-free bridge) → **M5 (rides P-GA-32 / P-GA-33,
//! GA-D8)**; the live Search/Refs/Notif/Agent/OLAP bindings behind the seam are a config swap at boot
//! (the in-memory models here have byte-for-byte the §4.4 / GA-D7 post-condition); this module touches
//! **NO new DB / object-store / cache / bus contract — no `--features integration` leg owed**.
//! **Mutation floor (P-GA-25 TESTS — the restriction-suppression-across-derived-stores path is
//! mandatory-core):** each derived store's [`restrict_fanout::DerivedStore::process`] suppression
//! branch (both polarities, storage retained either way) + the
//! [`restrict_fanout::RestrictFanOutDriver::fan_out_restrict`] 0-processing roll-up are the behavioral
//! core. `cargo mutants -p myelin-gdpr-service --file crates/myelin-gdpr-service/src/restrict_fanout.rs`
//! (2026-06-20): **47 mutants, 20 caught, 27 unviable, 0 missed** — EVERY behavioral mutant on the
//! mandatory-core path is CAUGHT (the per-store suppression branch both polarities, the
//! `processed_count`/`all_suppressed`/`all_rows_retained` GA-D7 readings, the per-`(tenant,subject)`
//! key, the shared-flag set/clear receipts).
//!
//! ## P-GA-15 (→ P-115) — the erasure ledger (10.8) + post-restore re-erasure + the crypto-shred-reaches-backups proof
//! [`erasure_ledger`] ships the **erasure ledger** ([`erasure_ledger::ErasureLedger`]) — the GDPR-owned,
//! **PII-free, NON-shred-erasable** record of every completed erasure (the opaque subject token + the
//! holders erased + the **destroyed key epochs** + the **cross-seam completion offset**). On a DSR
//! completion the fan-out driver ([`fanout::FanOutDriver::with_ledger`]) writes one
//! [`erasure_ledger::ErasureLedgerEntry`] **idempotently** (keyed on the DSR id — a worker restart
//! re-driving the same id does NOT duplicate). The ledger is itself a **recursive
//! [`myelin_gdpr::PersonalDataHolder`]** whose `erase` **RETAINS** the PII-free record (it holds no
//! PII and MUST survive to drive re-erasure — §2.3, the carve-out with the audit log) — the ONE holder
//! the per-tenant crypto-shred does NOT erase away. It drives **Storage's `post_restore_reerase`**
//! (11.5): a restore reads [`erasure_ledger::ErasureLedger::post_pit_records_after`] and re-erases
//! every subject erased AFTER the restore's PIT, so a restore never resurrects erased PII (§3.2 /
//! GD-14). **§1.2 ownership split:** Storage owns the restore MECHANISM (the `ReErasePass` +
//! `PostRestoreErasureLedger` seam, P-100); GDPR owns the LEDGER that drives it — this module. The
//! [`erasure_ledger::PostPitRecord`] field shape mirrors Storage's `ErasureRecord` exactly so the boot
//! wiring (`myelin-control-plane`, which depends on BOTH crates) is a 1:1 field copy; the **CDC pair**
//! (`crates/myelin-control-plane/tests/cdc_10_8_erasure_ledger_drives_reerase.rs`) proves the
//! provider (this ledger writes) ⇄ consumer (Storage re-erases from it) seam; the STOR-D4-GA-face /
//! STOR-D3-GA-face **drills** there emit the dated green artifacts (0 recoverable in backups; 0
//! resurrected after a restore). **Floors named:** the M1-scale drills re-run at CELL scale + the full
//! H1–H18 GA-D1 fan-out → **M5 P-GA-32 → P-505**; the durable Postgres `erasure_ledger` table
//! (excluded from the crypto-shred by construction) + the live WAL completion offset → the same
//! DB/cursor floor every M0/M1 in-memory store carries (P-007 / P-S12; on this floor the offset is the
//! monotone completion timestamp surrogate); the Merkle SEAL / audit hash-link of the completion fact
//! → **P-GA-20 → P-119**. **Mutation floor (P-GA-15 TESTS):** the
//! [`erasure_ledger::ErasureLedger::record_completion`] write (idempotent, keyed on the DSR id) and the
//! [`erasure_ledger::ErasureLedger::post_pit_records_after`] re-erasure-trigger read (the
//! `completed_at_offset > pit` selection) are mandatory-core; the `cargo mutants` score is in the
//! commit body.
//!
//! ## P-GA-16 (→ P-116) — the ONE free-text/immutable erasure posture (X-7) written ONCE (contract 10.9)
//! [`posture`] ships the **single canonical artifact** for the free-text / immutable-content erasure
//! posture ([`posture::CANONICAL_POSTURE`]) — the keystone X-7 deliverable. The same legal seam was
//! named five times (Git immutable bytes, CI log PII, Issues/Knowledge/Chat free-text); this is the
//! ONE platform-wide posture, instantiated per subsystem **BY REFERENCE, never restated five times**
//! (gdpr §7.4). The artifact states: the **structural floor** (the three §7.1 levers —
//! [`posture::StructuralLever`]: per-subject DEK crypto-shred (11.4) + pseudonym-map shred (4.8) +
//! `restrict` suppression (10.1), all built, no legal dependency); the **residual** (§7.2 —
//! third-party / immutable free-text PII authored by *others*, encrypted under the AUTHOR's DEK not
//! the subject's, so NOT crypto-shreddable by the subject's key — the documented limit, not a
//! defect); and the **ratified engineering posture** (§7.3 — the documented lawful-basis limit +
//! best-effort `rectify`/tombstone + the standing `restrict` guarantee). The residual ratification
//! is the **`[OPEN — LEGAL]`** tag ([`posture::LegalStatus::OpenLegal`]) — **ONE statement, not
//! five** — and the structural floor ships regardless ([`posture::ErasurePosture::structural_floor_ships`]).
//! [`posture::ErasurePosture::render`] emits the doc text the artifact GENERATES. The GATE is the
//! **architecture-test scaffolding** that the posture is a SINGLE source: a subsystem erasure
//! section must REFERENCE the anchor ([`posture::POSTURE_ANCHOR`]) and **never restate** the posture
//! ([`posture::reference_is_by_reference`] — rejects a section that contains a canonical marker
//! phrase, the X-7 anti-pattern). **Floors named:** the structural-floor PROOF on the M1 stores →
//! **P-GA-17 → P-117**; the pseudonymous-by-default commit-identity prerequisite for Git →
//! **P-GA-18 → P-118**; the audited history-rewrite erasure path → **M5 P-GA-35**; the per-subsystem
//! reference ASSERTIONS fire when the M3/M4 instances register references (Git first, the consumer
//! half of the 10.9 CDC pair → **P-GA-28 → P-256/P-257**; CI/Issues/Knowledge/Chat → P-GA-29/-31);
//! the residual lawful-basis ratification (`[OPEN — LEGAL]` → ratified) is **parallel-legal** (the
//! DPO ratifies; the structural floor ships regardless). **Mutation floor:** none — a documented
//! canonical artifact, not core logic (NAMED per the prompt TESTS); the one behavioral predicate
//! ([`posture::reference_is_by_reference`]) is unit-covered.
//!
//! ## P-GA-17 (→ P-117) — the structural erasure floor PROVEN on the M1 stores (contract 10.9 §7.1)
//! [`structural_floor`] PROVES the [`posture`] structural floor (§7.1) **working end-to-end on the
//! M1 stores** — the three levers, observable THROUGH a faithful M1-store model: **lever 1**
//! per-subject DEK crypto-shred ([`structural_floor::M1Store::erase_self_authored`] renders
//! self-authored free-text [`structural_floor::StoredContent::Unrecoverable`]); **lever 2**
//! pseudonym-map shred ([`structural_floor::shred_pseudonym_identity`] leaves the immutable bytes
//! holding ONLY the frozen `<pseudonym>@<tenant>.noreply` form, contract 4.8); **lever 3** the
//! genuinely-new **`restrict` suppression FLAG every M1 holder HONOURS** (the
//! [`structural_floor::RestrictRegistry`] plus the [`structural_floor::M1Store`] processing ops) —
//! while restricted, index/agent-read/analyse/notify are
//! [`structural_floor::Processed::Suppressed`] while storage is RETAINED
//! ([`structural_floor::M1Store::fetch_stored`] still returns the content), reversible (§4.4). Before
//! P-117 the holders RECORDED a restrict receipt but no store HONOURED it — the floor was stated,
//! not proven. The residual (a third-party mention under the AUTHOR's DEK) is classified
//! ([`structural_floor::classify_residual`] → [`structural_floor::LeverCoverage::RestrictSuppressOnly`])
//! as `restrict`-suppressed, NEVER crypto-shredded by the subject's key — the documented limit from
//! P-GA-16 / §7.2. The GATE drill (`tests/ga_d7_m1_restrict_honoured.rs`) observes the floor
//! end-to-end. **Floor named:** the full restriction-into-derived-stores proof (GA-D7 — the flag into
//! Search/Refs/Notif/Agents/OLAP) → **M2 P-GA-25 → P-152**; the live store/KMS/Identity bindings are
//! the same DB/KMS floor every M0/M1 store carries (P-007 / P-S12) — this module composes already-
//! shipped seams and touches NO new DB/object-store/cache/bus contract (no `--features integration`
//! leg owed). **Mutation floor (P-GA-17 TESTS):** the [`structural_floor::RestrictRegistry::is_restricted`]
//! flag, the [`structural_floor::M1Store`] suppression branch, and [`structural_floor::classify_residual`]
//! are mandatory-core; the `cargo mutants` score is in the commit body.
//!
//! ## P-GA-22 (→ P-149) — the retention engine: tightest-policy-wins merge + legal-hold-aware suspend (contract 10.5)
//! [`retention`] ships the **retention engine** ([`retention::RetentionEngine`]) — the retention leg
//! of contract 10.5 (§5.1). Two responsibilities: (1) the **tightest-policy-wins merge**
//! ([`retention::RetentionEngine::effective_retention`]) — given the per-field [`myelin_gdpr::
//! RetentionClass`] (G3) inputs for a `(category, tenant, store)`, each tagged with which
//! [`retention::RetentionSource`] named it, it picks the **most restrictive** policy that **does not
//! violate a legal-retention floor**, deterministically, and **records which input won**
//! ([`retention::EffectiveRetention::winning_source`] — a tenant 30-day policy beats a 90-day
//! default; a lawful 6-month floor — an [`myelin_gdpr::RetentionClass::AuditCarveOut`] — clamps a
//! tenant "delete immediately" UP, recorded as the floor winner); (2) the **legal-hold-aware
//! suspend-don't-delete expiry** ([`retention::RetentionEngine::expire`]) — an elapsed field is
//! expired via the §3 erasure mechanisms (the SAME canonical-order holder fan-out the DSR erase
//! uses) UNLESS an active `legal_hold` over the scope SUSPENDS it (read through the EXISTING G4
//! [`fanout::LegalHoldRegistry`], P-GA-12 — fail-safe-to-suspend; **0 held-scope deletions**;
//! resumes on hold-lift). It REUSES the hold gate + the holder fan-out wholesale (EI-01 §7 coherence
//! — no second hold registry, no re-implemented fan-out). **GA-D6** (set a hold → submit an expiry →
//! deferred-by-hold, 0 held-scope deletions → lift → resumes) emits its dated green artifact in
//! `tests/ga_d6_retention_legal_hold.rs`; the CDC pair is `tests/cdc_10_5_retention_engine.rs`.
//! **Floors named:** GA-D6 runs at M2 scale here; it re-confirms at CELL scale → **M5 P-GA-35**; the
//! consent / sub-processor registries + the `transfer_allowed` gate (the rest of 10.5) → **P-GA-23
//! → P-150**; the durable Postgres `retention_policy` (G3) table + the periodic expiry SWEEP
//! scheduler (the `myelin-flow` wheel) → the same DB / timer floor every M0/M1 store carries (P-007
//! / P-S12 / the P-GA-21 wheel — a config wire, not a code change). **Mutation floor (P-GA-22
//! TESTS):** the tightest-wins merge + the legal-hold suspend paths are mandatory-core; the
//! `cargo mutants` score is recorded in the [`retention`] module note + the commit body. **No
//! `--features integration` leg owed:** the engine is a pure decision + a holder-fan-out driver over
//! already-shipped in-memory seams — it touches NO new DB / object-store / cache / bus contract.
//!
//! ## P-GA-23 (→ P-150) — the consent registry + the sub-processor registry + the `transfer_allowed` gate (contract 10.5)
//! [`registries`] ships the **consent / sub-processor / transfer-gate legs of 10.5** (§5.2 / §5.3),
//! the rest of contract 10.5 after the retention leg (P-GA-22): (1) the **consent registry (G5)**
//! ([`registries::ConsentRegistry`]) — versioned + timestamped + granular + withdrawable +
//! per-subject-keyed; a withdrawal **propagates** ([`registries::ConsentRegistry::withdraw`] →
//! [`registries::WithdrawalEffect`]) — it stops the path AND, for a controller-posture consent-only
//! activity, **may trigger deletion** (carrying the [`myelin_gdpr::EraseScope`] the caller drives the
//! EXISTING holder fan-out over — no second erase path); (2) the **sub-processor registry (G6)**
//! ([`registries::SubProcessorRegistry`]) — versioned + region + DPA ref + the change-notification /
//! **objection workflow** ([`registries::SubProcessorRegistry::object`]); (3) the **`transfer_allowed`
//! gate** ([`registries::TransferGate::transfer_allowed`]) — **deny extra-EU by default** + admit
//! within-EU/EEA (the structural boundary is [`registries::is_eea_region`], fail-closed on an unknown
//! region), the SAME policy the §5.3 outbound push-mirror residency gate reads (the future real-LLM
//! backend is one such gated, EU-preferring, swappable adapter). It reuses the EXISTING G5 consent
//! DEK holder ([`holders::GdprOwnStoreHolder`], P-GA-05) — it does NOT re-define the key path. The
//! green artifact is [`registries::TransferGate::extra_eu_denial_count`] (0 default extra-EU
//! transfers slip through). **Floors named:** the outbound-mirror gate's POLICY ships here; the Git
//! mirror SEAM it gates is M3/M4 and the gate is PROVEN end-to-end at **M5 → P-GA-35 (GA-11)**; the
//! durable Postgres `consent` (G5) / `subprocessor_registry` (G6) tables are the same DB floor every
//! M0/M1 store carries (P-007 / P-S12 — in-memory model with byte-for-byte §5.2 semantics, a config
//! wire); the DPA legal-sufficiency ratification is **`[OPEN — LEGAL]`** (engineering carries the
//! `dpa_ref` + region + objection workflow; counsel ratifies). **Mutation floor (P-GA-23 TESTS — the
//! `transfer_allowed` deny-by-default + the consent-withdrawal-propagation paths are mandatory-core):**
//! the `cargo mutants` score is recorded in the [`registries`] module note + the commit body. **No
//! `--features integration` leg owed:** the registries + the gate are pure in-memory decision models
//! over already-shipped seams — they touch NO new DB / object-store / cache / bus contract.
//!
//! ## P-GA-28 (→ P-257) — the Git pseudonymous-commit instance of X-7 (10.9 BY REFERENCE) + GIT-D2
//! [`git_instance`] ships the **Git instance of the ONE posture — BY REFERENCE** (§7.4), the FIRST
//! real subsystem register that fires the P-GA-16 by-reference GATE scaffolding green. It (1)
//! registers the Git erasure-section reference ([`git_instance::GIT_INSTANCE`]) that CITES the
//! canonical anchor ([`posture::POSTURE_ANCHOR`]) and **never restates** the posture
//! ([`git_instance::git_section_references_posture`] — the consumer half of the 10.9 CDC pair,
//! completing the P-GA-16/P-GA-18 stubs); (2) confirms **GIT-D2's residual == the ONE platform-posture
//! residual** ([`git_instance::git_residual_is_the_one_posture`] — the Git residual IS the canonical
//! [`posture::CANONICAL_POSTURE`]`.residual`, confirmed equal, never re-described); and (3) makes the
//! **P-GA-18 commit-identity prerequisite FIRE over Git's REAL commit codec** — the recorded obligation
//! (*Git's M3 commits hold only `<pseudonym>@<tenant>.noreply`*) is now enforced over
//! [`myelin_git::commit::Commit::canonical_bytes`] (pseudonymous-by-construction, GIT-P25) by the
//! verdict scaffold [`commit_prerequisite::commit_actor_holds_only_pseudonym`], called via
//! [`git_instance::pseudonym_actor_lines_pass_the_prerequisite`]. The GIT-D2 drill
//! (`tests/git_d2_pseudonymous_commit.rs`) is the dated green artifact: erase an author → 0 recoverable
//! real identity in the immutable bytes, residual == the ONE posture, crypto-shred reaches backups; the
//! P-GA-18 architecture test PASSES on the live codec. It REUSES the canonical posture / the
//! by-reference predicate / the pseudonym-verdict / the Git H1 holder ([`producer_holders::GitDbHolder`])
//! WHOLESALE — no restatement, no second predicate. **Floor named:** the audited **history-rewrite
//! erasure path** (the rare commit-body expunge, with the disruptive changed-hash consequence) →
//! **M5 P-GA-35 (GA-10)** ([`git_instance::HISTORY_REWRITE_FLOOR_PROMPT`]); the live Git `erase`
//! binding is the config swap P-GA-27 named. **No `--features integration` leg owed:** this confirms a
//! reference + fires a pure-bytes architecture test over the in-process commit codec — it touches NO
//! new DB / object-store / cache / bus contract. **Mutation floor (P-GA-28 TESTS — the
//! pseudonym-form-only-in-commit-bytes check is mandatory-core):** the verdict
//! [`commit_prerequisite::commit_actor_holds_only_pseudonym`] is killed by {a pseudonym actor passes,
//! every real-identity actor fails} over fixtures AND over Git's real `canonical_bytes`; Git's own
//! `erase`-impl floor is owned by Git (GIT-P25). `cargo mutants` score recorded in the commit body.
//!
//! ## P-GA-29 (→ P-332) — the CI consumer holder (H2) + the per-subject CI-log DEK crypto-shred reach + the CI instance (CI-D3)
//! [`ci_instance`] ships the **CI consumer holder (H2)** — the orchestration leg of 10.1 over the CI
//! subsystem (the `erase` IMPL is CI's; GDPR REGISTERS + CALLS it): (1) **registers H2** (CI + log
//! segments) into the data map ([`ci_instance::ci_holder_schemas`] — the data-map diff surfaces it,
//! no holder-without-map drift); (2) **the fan-out reaches it** at its
//! [`ci_instance::ci_phase_of`] phase ([`orchestration::CanonicalErasePhase::CryptoShredDek`] — the CI
//! log free-text is a per-subject-DEK holder) via [`ci_instance::CiHolderRegistration::register_ci`];
//! (3) the **per-subject CI-log DEK crypto-shred reaches isolable log-segment PII**
//! ([`ci_instance::CiLogHolder`] — a subject erase destroys exactly that subject's per-subject CI-log
//! DEK (the C1/P5 reach shipped storage-side in P-329 / P-ST-27), 0 dangling leak incl. backups, while
//! a different subject's CI log AND the per-tenant FALLBACK survive; a tenant offboarding destroys the
//! per-tenant fallback — the honest per-subject-where-isolable / per-tenant-fallback split; the
//! run-graph structure survives in both); and (4) the **CI instance of the ONE posture BY REFERENCE**
//! ([`ci_instance::CI_INSTANCE`] cites [`posture::POSTURE_ANCHOR`] + never restates — the SAME
//! [`posture::reference_is_by_reference`] predicate the Git instance fired, the consumer half of the
//! 10.9 CDC pair for CI). **CI-D3** (erase fans to CI → isolable log PII destroyed per-subject,
//! per-tenant fallback for non-isolable, structure survives, 0 dangling leak incl. backups) emits its
//! dated green artifact in `tests/ci_d3_ci_holder_erasure.rs`; the CDC pair for 10.1 is
//! `tests/cdc_10_1_ci_holder.rs`. It REUSES the orchestrator / the canonical phase / the crypto-shred
//! KMS / the by-reference predicate WHOLESALE (EI-01 §7 coherence — no second orchestrator, no
//! re-defined posture). **Floors named:** the per-subject-where-isolable / per-tenant-fallback split
//! is the honest answer (named); the **Issues (H3) + Chat (H5) consumer holders** over this SAME
//! pattern → **P-GA-30 → P-333** ([`ci_instance::CONSUMER_HOLDER_FOLLOW_ON`]); the live CI `erase`
//! binding behind the seam is a config swap at boot (the per-subject CI-log DEK mechanism is
//! `myelin-storage`'s, its OWN live-stack integration proof owned storage-side, P-329 / STOR-D4-C1).
//! **No `--features integration` leg owed:** this composes already-shipped in-memory seams and touches
//! NO new DB / object-store / cache / bus contract. **Mutation floor (P-GA-29 TESTS — the
//! per-subject-where-isolable / per-tenant-fallback selection path is mandatory-core):**
//! [`ci_instance::CiLogHolder::erase`]'s scope selection (subject ⇒ per-subject DEK; tenant ⇒
//! per-tenant fallback) + [`ci_instance::ci_phase_of`] + [`ci_instance::CiHolderRegistration::register_ci`]
//! are the behavioral core; the `cargo mutants` score is in the commit body.

//! ## P-GA-31 (→ P-334) — the worklog/productivity Behavioural classification (OQ-H) + the works-council trigger + the SpecialCategory→DPIA route (contract 10.2)
//! [`worklog`] ships the **OQ-H worklog/productivity/estimate classification** (gdpr §2.4) — the
//! LAST consumer-classification follow-on, after which **all H1–H18 holders exist** (the GA-D1
//! precondition → M5 P-GA-32). Four structural parts, three composed from already-shipped seams +
//! one genuinely new. FIRST, the **restricted-by-default classification is STRUCTURAL** — shipped in
//! `myelin-gdpr` as the `data_role_default = Restricted` registry tag
//! ([`myelin_gdpr::PersonalDataField::is_restricted_by_default`]), applied to the Issues
//! `worklog_seconds` / `story_points` fields; the macro now CAPTURES it (was forward-compat-ignored).
//! SECOND, **excluded from cross-individual analytics by default** — [`worklog::WorklogAnalyticsGate`]'s
//! flipped default-DENY (a restricted-by-default field is denied cross-individual analytics/agent-use
//! UNLESS an explicit per-subject opt-in is recorded; an ordinary field is allowed) — the GA-D7
//! worklog face. THIRD, **per-individual rollups OFF by default + the works-council consultation
//! trigger** — [`worklog::RollupEnablement`] (OFF by default; enabling surfaces a
//! [`worklog::WorksCouncilTrigger`] — a SURFACED obligation, never an auto-decision, §8). FOURTH, the
//! **SpecialCategory worklog field → DPIA route** — REUSES the [`myelin_gdpr::DpiaRouter`] (P-GA-08)
//! verbatim. Plus the **build-data-as-LLM-training foreclosure** ([`worklog::BUILD_TRAINING_FORECLOSURE`]
//! and the architecture test `worklog::tests::build_data_as_llm_training_has_no_code_path` — the
//! foreclosure is the ABSENCE of a training-feed surface). **Floors named:** the worklog
//! `basis = TBD_LEGAL` is the `[OPEN — LEGAL]` residual ([`worklog::WORKLOG_BASIS_RESIDUAL`] — counsel
//! ratifies special-category vs elevated + the per-jurisdiction works-council trigger; the structural
//! floor ships regardless); all H1–H18 now exist ([`worklog::ALL_HOLDERS_EXIST_FOR`] → M5 P-GA-32).
//! It REUSES the data-map registry / the OLAP-restrict chokepoint / the DPIA router WHOLESALE (EI-01
//! §7 — no second router, no re-detected "is this worklog?" by field name; the MAP drives it). **No
//! `--features integration` leg owed:** this composes already-shipped in-memory seams + reads the
//! compile-time registry and touches NO new DB / object-store / cache / bus contract. **Mutation
//! floor (P-GA-31 TESTS — the restricted-by-default + the rollup-off-by-default + the
//! works-council-trigger-surfacing paths are mandatory-core):** [`worklog::WorklogAnalyticsGate::
//! cross_individual_allowed`] (both polarities), [`worklog::RollupEnablement::is_enabled`] (OFF
//! default), and [`worklog::RollupEnablement::enable`]'s trigger emission; the `cargo mutants` score
//! is in the commit body.

pub mod agent_trace_seam;
pub mod audit;
pub mod audit_proofs;
pub mod ci_instance;
pub mod commit_prerequisite;
pub mod datamap;
pub mod derivative_erasure;
pub mod diffgate;
pub mod dsr;
pub mod dsr_timer;
pub mod ediscovery;
pub mod erasure_ledger;
pub mod fanout;
pub mod full_fanout;
pub mod git_instance;
pub mod history_rewrite;
pub mod holders;
pub mod issues_chat_instance;
pub mod multi_cell;
pub mod orchestration;
pub mod posture;
pub mod producer_holders;
pub mod registries;
pub mod restrict_fanout;
pub mod retention;
pub mod structural_floor;
pub mod tenant_ops;
pub mod worklog;

pub use agent_trace_seam::{
    agent_trace_phase, trace_is_distinct_from_audit, AgentTraceHolderSeam, AGENT_TRACE_ERASABLE,
    AGENT_TRACE_HOLDER_ID, AGENT_TRACE_IMPL_PROMPT, AUDIT_LOG_ERASABLE,
};
pub use audit::{AuditConsumer, AuditEntry, AuditLog, Minimised, Outcome, AUDIT_APPEND_LAG};
pub use audit_proofs::{
    serialize_sth_commitment, verify_consistency, verify_inclusion, AuditAuthority, CellSigningKey,
    ConsistencyProof, InclusionProof, NotaryWitness, SignedTreeHead, SigningKey, Witness,
    WitnessAttestation, DSR_SEAL_ACTION, STH_PUBLISH_AGE,
};
pub use ci_instance::{
    ci_holder_schemas, ci_phase_of, ci_registrations, ci_residual, ci_section_references_posture,
    CiHolderRegistration, CiLogHolder, CiLogModel, CI_DB, CI_INSTANCE, CI_SUBSYSTEM,
    CONSUMER_HOLDER_FOLLOW_ON,
};
pub use commit_prerequisite::{
    commit_actor_holds_only_pseudonym, verdict_for, CommitActorVerdict, CommitIdentityPrerequisite,
    COMMIT_IDENTITY_PREREQUISITE, M3_ENFORCEMENT_PROMPT, PREREQUISITE_CONTRACT_ROW,
    PREREQUISITE_GRAMMAR, PREREQUISITE_RECORDED_ON,
};
pub use datamap::{
    data_map, ropa, ropa_for_tenant, tagged_field_count, HolderSchema, Inventory, InventoryEntry,
    ProcessingActivities, ProcessingActivity, DATA_MAP_ENTRY_COUNT, DATA_MAP_HOLDER_COUNT,
};
pub use derivative_erasure::{
    derivative_holder_ids, derivative_phase_of, DerivativeEraseReceipt, DerivativeErasureDriver,
    NotifHistoryHolder, NotifHistoryModel, RectifyOutcome, RefsGraphHolder, RefsGraphModel,
    RefsResolve, SearchIndexHolder, SearchIndexModel, DERIVATIVE_ERASE_FANOUT_COVERAGE,
    ERASED_USER,
};
pub use diffgate::{
    check_against_baseline, diff, CommittedBaseline, DataMapDiff, GateVerdict, Reclassification,
    COMMITTED_BASELINE_FINGERPRINT,
};
pub use dsr::{
    resolve_checklist_from_map, ChecklistItem, Dsr, DsrError, DsrId, DsrKind, DsrOrchestrator,
    DsrRequestView, DsrState, DsrStatus, Initiator, MerkleProvenBundle, Posture, DSR_DEADLINE_SECS,
    DSR_STATE,
};
pub use dsr_timer::{
    DsrDeadlineTimer, DsrDeadlineWarning, DsrTimerWheel, TimerEntrySnapshot, TimerError,
    DSR_DEADLINE_MARGIN,
};
pub use ediscovery::{
    EDiscoveryBundle, EDiscoveryExporter, EDiscoveryRecord, EDiscoveryScope,
    EDISCOVERY_EXPORT_RECORDS,
};
pub use erasure_ledger::{
    DestroyedKeyEpoch, ErasureLedger, ErasureLedgerEntry, PostPitRecord, ERASURE_LEDGER_ENTRIES,
    ERASURE_LEDGER_STORE,
};
pub use fanout::{
    DsrCompletionReceipt, FanOutDriver, FanOutOutcome, HoldScope, HoldVerdict, LegalHoldRegistry,
    LEGAL_HOLD_ACTIVE_COUNT,
};
pub use full_fanout::{
    FullFanOutCoverage, GaD1Certificate, GaD1Gap, Holder, HolderErasure, HolderReach,
    ERASURE_FANOUT_COVERAGE as FULL_FANOUT_ERASURE_COVERAGE,
};
pub use git_instance::{
    git_residual, git_residual_is_the_one_posture, git_section_references_posture,
    pseudonym_actor_lines_pass_the_prerequisite, residual_is_the_one_posture,
    section_references_posture, GIT_INSTANCE, GIT_SUBSYSTEM, HISTORY_REWRITE_FLOOR_PROMPT,
};
pub use history_rewrite::{
    HistoryRewriteActivity, HistoryRewriteReceipt, HistoryRewriteRequest, PhaseReceipt,
    RewritePhase, HISTORY_REWRITE_ACTION, HISTORY_REWRITE_FIRST_CLASS_PROMPT,
};
pub use holders::{
    gdpr_owned_holder_ids, AuditCarveOutHolder, CryptoShredKms, GdprOwnStoreHolder,
    InMemoryShredKms, ShredKeyClass, ShredKeyHandle, AUDIT_CARVE_OUT_STORE, GDPR_OWN_STORE,
};
pub use issues_chat_instance::{
    chat_residual, chat_section_references_posture, issues_chat_holder_schemas,
    issues_chat_phase_of, issues_chat_registrations, issues_residual,
    issues_section_references_posture, ChatCascadeReceipt, ChatStoreHolder, ChatStoreModel,
    IssuesCascadeReceipt, IssuesChatCascadeDriver, IssuesStoreHolder, IssuesStoreModel, CHAT_DB,
    CHAT_INSTANCE, CHAT_SUBSYSTEM, ISSUES_DB, ISSUES_INSTANCE, ISSUES_SUBSYSTEM,
    WORKLOG_CLASSIFICATION_FOLLOW_ON,
};
pub use multi_cell::{
    MemberCellSet, MultiCellCertificate, MultiCellCoverage, MultiCellFanOut, MultiCellGap,
    PerCellReceipt,
};
pub use orchestration::{
    canonical_phase_of, holder_ids, CanonicalErasePhase, EraseChecklist, HolderReceipt,
    RegisteredHolder, SeamHolder, UpstreamHolderOrchestrator, CRYPTO_SHRED_LAG,
    ERASURE_FANOUT_COVERAGE,
};
pub use posture::{
    reference_is_by_reference, restatement_markers, ErasurePosture, LegalStatus, StructuralLever,
    SubsystemReference, CANONICAL_POSTURE, POSTURE_ANCHOR, POSTURE_CONTRACT_ROW,
};
pub use producer_holders::{
    producer_holder_ids, producer_holder_schemas, producer_phase_of, producer_registrations,
    AgentTraceModel, GitDbHolder, KnowledgeAgentTraceHolder, KnowledgeStoreHolder,
    KnowledgeStoreModel, ProducerHolderRegistration,
};
pub use registries::{
    is_eea_region, ConsentRecord, ConsentRegistry, SubProcessor, SubProcessorRegistry,
    TransferGate, TransferVerdict, WithdrawalBasis, WithdrawalEffect, CONSENT_WITHDRAWALS,
    SUBPROCESSOR_OBJECTIONS, TRANSFER_GATE_EXTRA_EU_DENIALS,
};
pub use restrict_fanout::{
    restrict_holder_ids, DerivedProcessed, DerivedProcessing, DerivedRestrictVerdict, DerivedStore,
    DerivedStoreHolder, RestrictFanOutDriver, RestrictFanOutOutcome,
    RESTRICT_FANOUT_PROCESSING_SUPPRESSED,
};
pub use retention::{
    legal_floor, platform_default, tenant_delete_immediately, tenant_window, EffectiveRetention,
    ExpiryError, ExpiryOutcome, RetentionEngine, RetentionInput, RetentionSource,
    RETENTION_EXPIRY_RUNS, RETENTION_HELD_SCOPE_DELETIONS,
};
pub use structural_floor::{
    classify_residual, shred_pseudonym_identity, Authorship, LeverCoverage, M1Store, Processed,
    Processing, RestrictRegistry, ShreddedIdentity, StoredContent,
};
pub use tenant_ops::{OffboardingCertificate, TenantDsrError, TenantDsrSurface};
pub use worklog::{
    RollupEnablement, WorklogAnalyticsGate, WorksCouncilTrigger, ALL_HOLDERS_EXIST_FOR,
    BUILD_TRAINING_FORECLOSURE, WORKLOG_BASIS_RESIDUAL, WORKLOG_CROSS_INDIVIDUAL_DENIED,
    WORKS_COUNCIL_TRIGGERS_SURFACED,
};

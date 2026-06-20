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

pub mod audit;
pub mod commit_prerequisite;
pub mod datamap;
pub mod diffgate;
pub mod dsr;
pub mod erasure_ledger;
pub mod fanout;
pub mod holders;
pub mod orchestration;
pub mod posture;
pub mod structural_floor;
pub mod tenant_ops;

pub use audit::{
    AuditConsumer, AuditEntry, AuditLog, Minimised, Outcome, AUDIT_APPEND_LAG,
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
pub use diffgate::{
    check_against_baseline, diff, CommittedBaseline, DataMapDiff, GateVerdict, Reclassification,
    COMMITTED_BASELINE_FINGERPRINT,
};
pub use dsr::{
    resolve_checklist_from_map, ChecklistItem, Dsr, DsrError, DsrId, DsrKind, DsrOrchestrator,
    DsrRequestView, DsrState, DsrStatus, Initiator, MerkleProvenBundle, Posture, DSR_DEADLINE_SECS,
    DSR_STATE,
};
pub use erasure_ledger::{
    DestroyedKeyEpoch, ErasureLedger, ErasureLedgerEntry, PostPitRecord, ERASURE_LEDGER_ENTRIES,
    ERASURE_LEDGER_STORE,
};
pub use fanout::{
    DsrCompletionReceipt, FanOutDriver, FanOutOutcome, HoldScope, HoldVerdict, LegalHoldRegistry,
    LEGAL_HOLD_ACTIVE_COUNT,
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
pub use posture::{
    reference_is_by_reference, restatement_markers, ErasurePosture, LegalStatus, StructuralLever,
    SubsystemReference, CANONICAL_POSTURE, POSTURE_ANCHOR, POSTURE_CONTRACT_ROW,
};
pub use structural_floor::{
    classify_residual, shred_pseudonym_identity, Authorship, LeverCoverage, M1Store, Processed,
    Processing, RestrictRegistry, ShreddedIdentity, StoredContent,
};
pub use tenant_ops::{OffboardingCertificate, TenantDsrError, TenantDsrSurface};

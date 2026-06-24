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
//! - **The [`blob::BlobStore`] (P-ST-03 / 11.2)** is now IMPLEMENTED in [`blob`]: the frozen
//!   content-addressed `put/get/head/delete` trait + the fs-backed floor
//!   ([`blob::FsBlobStore`]), with **BLAKE3 hash-on-write** (a self-describing multihash
//!   prefix so SHA-256 coexists), **address-by-plaintext-hash within a per-tenant keyspace,
//!   store ciphertext** (per-tenant dedup, no cross-tenant share), and **re-hash-on-read
//!   integrity** (corrupt object → `blob_integrity_fail` + 0 silent serve, the STOR-D7
//!   floor). Floors named in [`blob`]: the per-blob content-key WRAP is the
//!   [`blob::IdentityWrap`] floor → real DEK wrap at **P-ST-08 (P-095)**; the fs backing →
//!   **object-store (MinIO/Ceph) at P-ST-30 (P-636)**; the BlobStore crypto-shred DSR body →
//!   GDPR M1 (P-ST-09). The BlobStore registers as a holder via the [`holder`] seam.
//! - **The [`migration::OnlineMigrationRunner`] (P-ST-05 / contract 1.5)** is now IMPLEMENTED in
//!   [`migration`]: the forward-only ONLINE migration runner for the OLTP tier — it admits ONLY the
//!   online shape (expand→backfill→contract), rejecting a **contract-before-backfill ordering** at
//!   runtime (the P-ST-05 GATE) as well as a destructive `DROP`, a blocking `ALTER` on a declared-
//!   hot table, and a `Plain` migration touching a hot table (a hot-table change MUST use the online
//!   path). It RECONCILES with the substrate boot-time runner (P-S15/P-032 in `myelin-substrate`):
//!   the substrate owns the forward-only refusal mechanism, this adds the ordering enforcement the
//!   substrate runner lacks; the two share the contract-1.5 phase/hot-table vocabulary (re-stated,
//!   not imported, because the crate DAG forbids a `myelin-storage → myelin-substrate` edge — see
//!   the DEVIATION note above). **Floor named in [`migration`]:** STOR-D8 (online migration under
//!   load on the RESTORED copy, lock-budget measured) is the M2 follow-on **P-ST-21 (global P-126)**
//!   — it needs the restored copy restore-verify produces; here the runner exists + admits only the
//!   online shape at unit scale. The mutation floor on the ordering gate is ≥ 80% (mandatory-core).
//! - **The three-level KMS hierarchy + the fail-static posture (P-ST-06 / contract 11.3 — global
//!   P-058)** is now IMPLEMENTED in [`kms`] + [`kms_failstatic`]: the [`kms::KmsEngine`] holds the
//!   L0 cell root, the L1 per-(tenant,region) KEKs, and the L2 DEKs (AES-256-GCM, per-tenant for
//!   bulk + per-subject for the individual-erasure classes), stored ONLY envelope-wrapped
//!   ([`kms::WrappedDek`]). The frozen [`kms::PiiKeyRef`] (`kms://<tenant>/<dek-epoch>/<class>`)
//!   travels with every ciphertext; [`kms::KmsEngine::rotate_kek`] is envelope re-wrap (O(keys),
//!   not O(data), forward-only); [`kms::KmsEngine::destroy_kek`]/`destroy_dek` are the
//!   crypto-shred levers (a destroyed key renders its DEKs unrecoverable and is EXCLUDED from
//!   [`kms::KmsEngine::backup_snapshot`] — it stays dead across a restore, §7.5). The
//!   [`kms_failstatic::KmsReadPath`] over the [`kms::KmsAdapter`] seam gives the STOR-D6
//!   availability posture: a transient KMS outage → resolved-DEK reads survive a bounded TTL; a
//!   sustained hard-down → not-ready + shed ([`kms_failstatic::KmsReadiness::NotReady`]); **0
//!   fail-open** (no path returns a DEK without a fresh resolve or an in-budget cache). **Floors
//!   named in [`kms`]:** the `KeyOrigin` trait (platform-managed | BYOK | HYOK + the
//!   `can_derive_plaintext_index()=false` structural HYOK enforcement) — formerly the sibling
//!   **P-ST-07 (global P-094)** — is now SHIPPED in [`key_origin`] and FRONTS this engine (it
//!   calls [`kms::KmsEngine::wrap_dek_material`]/`unwrap_dek_material`); the OLTP/blob ENCRYPTION wiring (classify-driven
//!   key choice, the real per-blob content-key wrap) is **P-ST-08 (global P-095)**; the
//!   per-content-class HYOK POLICY + the KMIP/external-key-store adapter + HYOK-as-Schrems-III
//!   (GD-7) are `[OPEN → P6/LEGAL]` named follow-ons (mechanism ships; policy → counsel/DPO); the
//!   HSM/Shamir-split L0 backing is the production-hardening follow-on (the SHAPE — root wraps
//!   KEKs, never exported — is complete). The mutation floor on the wrap/unwrap/destroy +
//!   fail-open-prevention path is ≥ 80% (mandatory-core).
//! - **The `KeyOrigin` trait + the structural HYOK enforcement (P-ST-07 / contract 11.3 — global
//!   P-094)** is now IMPLEMENTED in [`key_origin`]: the [`key_origin::KeyOrigin`] trait
//!   (`wrap`/`unwrap`/`can_derive_plaintext_index`/`destroy`, copied byte-exact from storage.md §6)
//!   puts platform-managed | BYOK | HYOK behind ONE trait, FRONTING the P-058
//!   [`kms::KmsEngine`] (via [`kms::KmsEngine::wrap_dek_material`]/`unwrap_dek_material`). The
//!   three origins: [`key_origin::PlatformManaged`] (Myelin holds the key → full search/agents,
//!   `can_derive_plaintext_index()=true`); [`key_origin::Byok`] (the customer's key wraps DEKs
//!   under a customer-key path — same capability while the key is live, `=true`, plus the
//!   instant-shred-by-revoke lever); [`key_origin::Hyok`] (the customer holds the key OUT of
//!   Myelin's reach — `unwrap` is a CALL OUT via the [`key_origin::HyokKeyService`] seam that **may
//!   DENY** ([`key_origin::KeyOriginError::HyokDenied`]), Myelin never holds the plaintext key, and
//!   **`can_derive_plaintext_index()=FALSE` — the STRUCTURAL HYOK enforcement**). The
//!   [`key_origin::IndexAdmission`] seam is the call shape Search/Agent consult before
//!   building a plaintext-derived index ([`key_origin::IndexAdmission::for_origin`] → `SkipHyok` for
//!   HYOK) — *you cannot index what you cannot decrypt*, enforced by code, not by review. The §6
//!   per-class telemetry is [`key_origin::KeyOriginTelemetry`] (`can_derive_plaintext_index` per
//!   origin). It REUSES the P-058 [`kms::WrappedDek`]/[`kms::DekHandle`]/[`kms::DekId`] (re-exported
//!   as [`key_origin::KeyId`]) — NEVER a second key type (the §6 `Dek`/`KeyId` names bind to the
//!   already-frozen engine types; documented deviation, EI-01 §1). **Floors named in
//!   [`key_origin`]:** the per-content-class HYOK **POLICY** (which classes may be HYOK; the
//!   cross-artifact-reference-spanning case), the KMIP / external-key-store **adapter** (the real
//!   HYOK call-out — here an in-process customer-key-service stand-in proves the deny path), and
//!   HYOK-as-a-Schrems-III mitigation (GD-7) are `[OPEN → P6/LEGAL]` named follow-ons (mechanism
//!   ships; policy → counsel/DPO); the full Search/Agent skip drill **D-S10** lands WITH
//!   Search/Agent (this prompt ships the mechanism + the scoped HYOK check + the IndexAdmission
//!   seam they consult). The OLTP/blob ENCRYPTION wiring that drives origin selection per class is
//!   the sibling **P-ST-08 (global P-095)**. The mutation floor on the
//!   `can_derive_plaintext_index` branch + the HYOK deny path is mandatory-core (≥ 80%).
//! - **OLTP + blob envelope encryption wired + classify-driven key choice (P-ST-08 / contracts
//!   11.1 + 11.2 + 11.4 — global P-095)** is now IMPLEMENTED in [`encryption`]: it CLOSES the two
//!   floors named upstream. (1) The **plaintext-at-rest floor P-ST-01 named** is closed by
//!   [`encryption::ColumnCryptor`] — a personal-data column written through
//!   [`encryption::ColumnCryptor::encrypt`] is sealed under the classify-chosen DEK and stored as a
//!   ciphertext-only [`encryption::EncryptedColumn`] ([`encryption::ColumnCryptor::plaintext_at_rest_count`]
//!   is the `plaintext_at_rest_count == 0` telemetry the GATE asserts for tagged columns). (2) The
//!   **content-key-wrap floor P-ST-03 named** is closed by [`encryption::DekContentWrap`] — a real
//!   [`blob::ContentWrap`] that seals blob bytes under the tenant/per-subject DEK, REPLACING the
//!   [`blob::IdentityWrap`] plaintext floor (a localised swap; the content address stays
//!   plaintext-derived so nothing moves). (3) The **GD-4 classify-driven key choice (11.4)** is
//!   [`encryption::key_class_for`]: a field tagged `personal-data, erasure=subject`
//!   (`CryptoShred("subject_dek")`) is auto-wired to a per-subject DEK ([`kms::KeyClass::Subject`]),
//!   a bulk class (`Pseudonymise`/`PurgeReindex`/`CarveOut`/`CryptoShred("tenant_dek")`) to the
//!   per-tenant DEK ([`kms::KeyClass::Tenant`]) — *data whose erasure unit is the individual is
//!   keyed per-subject; data whose erasure is satisfied by pseudonymisation/tombstoning is keyed
//!   per-tenant* (§5.1). A subject-class tag with no subject is a LOUD
//!   [`encryption::KeyChoiceError::SubjectClassMissingSubject`] — **never a silent tenant-key
//!   downgrade** (which would lose the GD-4 individual-erasure lever). It REUSES the P-058
//!   [`kms::KmsEngine`] (the SAME engine rotation/crypto-shred reach — never a parallel key store),
//!   the [`kms::KeyClass`] vocabulary (the GDPR `CryptoShred(key_class)` tag and the KMS class speak
//!   one vocabulary), the [`blob::ContentWrap`] seam (P-047), and contract 10.2's
//!   [`myelin_gdpr::ErasureMethod`] tag (P-050) — never re-defined. The erase ALGORITHM that
//!   DESTROYS the chosen key is the sibling **P-ST-09 (global P-099)**. **Floor named in
//!   [`encryption`]:** the CI inline-PII log-segment per-subject DEK extension (C1) is the named
//!   **M4 follow-on (P-ST-27)** (the per-subject class today covers free-text/profile/chat-body/
//!   agent-memory). The mutation floor on the classify→key-choice routing + the
//!   ciphertext-at-rest property is mandatory-core (≥ 80%).
//! - **The crypto-shred `erase(subject, tenant)` six-step algorithm (P-ST-09 / contract 11.4 erase
//!   half — global P-099)** is now IMPLEMENTED in [`erase`]: [`erase::CryptoShredErase::erase`] runs
//!   the storage.md §5.2 algorithm in order — (1) pseudonym-map shred ([`erase::PseudonymShred`] →
//!   Id 4.8 `IdentityService::erase`), (2) `KMS.destroy(per_subject_DEK(tenant, subject))` (the step
//!   storage OWNS directly — [`kms::KmsEngine::destroy_dek`] on the subject's DEK, crypto-shredding
//!   the free-text/chat/profile/agent-memory ciphertext **live AND in backups by construction**,
//!   §7.5), (3) Search purge+reindex (the plaintext-derived EXCEPTION — [`erase::SearchPurge`]), (4)
//!   Refs tombstone ([`erase::RefsTombstone`]), (5) Bus erase ([`erase::BusErase`]), (6) record the
//!   erasure receipt to the audit/erasure-ledger holder ([`erase::ErasureLedgerSink`], 10.8). The
//!   algorithm is **idempotent** (re-erasing an already-erased subject is a NO-OP success, not an
//!   error — [`kms::KmsEngine::destroy_dek`] returns `false` on a second call, treated as success +
//!   flagged `re_run`) and a partial failure is a LOUD [`erase::EraseError`] (the erasure is recorded
//!   ONLY when every step succeeded — never "assume erased"). The cross-holder steps (1/3/4/5/6) are
//!   trait SEAMS the DSR orchestrator wires (storage cannot depend on the consumer subsystems Search/
//!   Refs without an upward DAG edge; Id/Bus/the-ledger are reached the same way for one uniform
//!   seam set); step 2 is owned in-crate. The [`erase::ErasureReceipt`] is the dated STOR-D4 artifact
//!   — `recoverable_in_backup == 0` is the `0 recoverable PII in any backup` gate reading (probed
//!   from [`kms::KmsEngine::backup_snapshot`], which already excludes a destroyed key, §7.5), with the
//!   `crypto_shred_lag_ms` telemetry. It REUSES the SAME P-058 [`kms::KmsEngine`] the encrypted
//!   columns/blobs resolve DEKs through (never a parallel key store — so the destroy reaches exactly
//!   the ciphertext those stores wrote) and the [`encryption::SubjectId`] vocabulary — never
//!   re-defined. **Floors named in [`erase`]:** the GD-4 granularity + structural GDPR floor is the
//!   sibling **P-ST-10 (global P-101)**; the git crypto-shred reach is **P-ST-24 (global P-253)**;
//!   the cross-holder reach COMPLETENESS (the every-holder D-S5 drill) is **P-ST-35 (M5)**; the
//!   post-restore RE-ERASURE (STOR-D3) is **P-ST-14 (global P-100)** (it replays the ledger this
//!   records into); the real seam bindings land with their subsystems (Id P-ID-20, Search M2, Refs
//!   M2, Bus P-092/P-093, ledger P-GA-15). The mutation floor on the six-step ordering + the
//!   idempotent short-circuit + the 0-recoverable-in-backup verify is mandatory-core.
//! - **GD-4 granularity wiring (complete) + the structural GDPR floor — by reference to X-7 (P-ST-10
//!   / contract 11.4 the GD-4 granularity + structural-floor half — global P-101)** is now
//!   IMPLEMENTED in [`gd4`]: it COMPLETES the P-099 [`erase`] algorithm's GD-4 half. (1) **GD-4
//!   granularity COMPLETENESS:** [`gd4::DataClass`] enumerates the storage.md §5.1 decision-rule table
//!   and [`gd4::DataClass::granularity`] routes EVERY class to its correct granularity, proven 0
//!   misrouted by [`gd4::assert_gd4_table_complete`] (the dated green artifact). This adds the THIRD
//!   granularity the DEK key-choice rule alone could not express — **tenant offboarding = the L1
//!   per-tenant KEK** ([`gd4::KeyGranularity::PerTenantKek`]), the level ABOVE the per-subject /
//!   per-tenant DEKs ([`gd4::KeyGranularity::PerSubjectDek`] / `PerTenantDek`). It is WIRED to the
//!   existing P-095 [`encryption::key_class_for`] rule via [`gd4::granularity_of_key_class`] +
//!   [`gd4::key_choice_granularity`] (the DEK key-choice and the granularity model agree by
//!   construction — never a second rule). (2) **The structural GDPR floor (X-7's structural half):**
//!   [`gd4::StructuralErasureFloor::verify`] proves the three guarantees that hold for ALL
//!   free-text/immutable content — the per-subject DEK crypto-shred lever renders content
//!   unrecoverable, the destroyed DEK is EXCLUDED from the backup snapshot (crypto-shred reaches
//!   backups by construction, §7.5 — `recoverable_in_backup == 0`), and the pseudonym-map shred reach
//!   is the Id step (P-099 step 1). It REUSES the SAME P-058 [`kms::KmsEngine`] the encrypted stores
//!   resolve through (never a parallel key store) and the P-099 [`erase::EraseHolders`] seam set (the
//!   structural reach IS the algorithm's reach — [`gd4::structural_reach_uses_erase_seams`], never a
//!   second reach). (3) **The residual handled BY REFERENCE (X-7), never restated:**
//!   [`gd4::RESIDUAL_POSTURE_REF`] is the ONLY thing Storage says about the residual — *"handled per
//!   the platform erasure posture in 00-reconciliation §X-7 (contract 10.9)"* —
//!   [`gd4::assert_no_local_residual_statement`] is the structural assertion the TESTS make that NO
//!   Storage-local residual statement exists (§5.3 / C7: one platform residual posture, not five).
//!   **Floors named in [`gd4`]:** the residual lawful-basis is `[OPEN → P6/LEGAL]` (counsel/DPO
//!   ratifies ONCE for all five subsystems — the structural floor ships regardless); the git
//!   crypto-shred reach (reflogs/bitmaps/pack-tier backups) is the Git **M3 reach P-ST-24 (global
//!   P-253)**; the CI inline-PII log-segment per-subject DEK wiring (C1) is the **M4 follow-on
//!   P-ST-27** (its GRANULARITY is fixed here as a named per-subject class). The mutation floor on the
//!   class→granularity routing is mandatory-core (≥ 80%).
//! - **Continuous WAL archiving + base backups + PITR (P-ST-11 / contract 11.5 — global P-059)**
//!   is now IMPLEMENTED in [`backup`]: [`backup::ContinuousArchiver`] ships sealed WAL segments
//!   off-host continuously (strictly forward, append-only) + takes periodic [`backup::BaseBackup`]s,
//!   giving a PITR window (base + archived WAL tail) and MEASURING the live RPO
//!   ([`backup::ContinuousArchiver::measure_rpo`] = committed − archived freshness) — the STOR-D2
//!   number asserted ≤ the `rpo_max_mins` threshold (≤ 5 min). [`backup::StoreTier::is_backed_up`]
//!   is the structural §7.1 rule (T1/T2/T3/T5 backed up; **T4 OLAP / T7 cache / derived indexes NOT
//!   backed up — rebuilt from source**; a derived tier in a [`backup::BackupSet`] is a type error).
//!   [`backup::ObjectTierBackup`] is the T2 versioned + in-region-replicated posture;
//!   [`backup::LogTierSeal`] is the T3 "sealed segments are immutable T2 blobs + range index in T1"
//!   binding; [`backup::BackupSet`] EXCLUDES crypto-shredded KMS keys (reusing
//!   [`kms::KmsEngine::backup_snapshot`] — §7.5, a shredded key stays dead across a restore). It
//!   REUSES the harness cross-seam assertion + the `RestoreRpoSecs` telemetry signal (P-056), the
//!   `seq` cross-seam cursor ([`coloc`], P-016), and the KMS exclusion ([`kms`], P-058) — never
//!   re-defined. **Floors named in [`backup`]:** the `restore(to_offset T)` + cross-seam rebuild is
//!   the sibling **P-ST-12 (global P-060)**; the CI-wired restore-verify GATE (STOR-D1) is
//!   **P-ST-13 (global P-061)**; the RTO / cell-kill half is **P-ST-14 (global P-100)**; the
//!   cell-scale RPO re-confirm is **P-ST-30 (M5)**; the real WAL-shipping driver is the P-S12/P-S15
//!   floor. The mutation floor on the crypto-shred-excluded-from-backup branch is mandatory-core.
//! - **`restore(to_offset T)` to the cross-seam consistency point (P-ST-12 / contract 11.5 — global
//!   P-060)** is now IMPLEMENTED in [`restore`]: [`restore::restore_to_offset`] lands every tier at
//!   ONE consistent point T (the per-aggregate outbox `seq` / event-log offset, the §7.3 cross-seam
//!   cursor [`coloc`] establishes): (1) PITR-restore OLTP to the rows whose `seq ≤ T` (reusing
//!   [`backup::ContinuousArchiver::pitr_reachable`] for reachability; a row past T is dropped); (2)
//!   verify every restored row's referenced [`blob::ContentHash`] is present in the restored object
//!   tier — a referenced-but-MISSING hash is the hard [`restore::RestoreError::DanglingBlobRef`]
//!   FAIL (the §7.3 silent-corruption case, the highest-bar silent-data-loss floor — it never
//!   silently passes); (3) **reindex derived stores FROM SOURCE up to T** through the live consumer
//!   replay ([`restore::ReindexFromSource`] — the ONLY rebuild path, never from a derived backup →
//!   *derived == source by construction*, EI-04 §5; consumers resume at T); (4) restore tenant KEKs
//!   EXCEPT any crypto-shredded since the backup (reusing [`kms::KmsEngine::backup_snapshot`], which
//!   already excludes a destroyed key — §7.5, a shredded key stays dead across the restore). It
//!   REUSES the backup machinery ([`backup`], P-059), the `seq` cursor ([`coloc`], P-016), the KMS
//!   exclusion ([`kms`], P-058), the [`blob::ContentHash`] address (P-047), and the harness
//!   cross-seam ASSERTION (`myelin_harness::restore::RestoredSnapshot::verify_cross_seam`, P-056,
//!   driven from the STOR-D1 drill) — never re-defined. **Floors named in [`restore`]:** the
//!   CI-wired restore-verify GATE (STOR-D1, the permanent gate) that DRIVES this restore is the
//!   sibling **P-ST-13 (global P-061)**; the post-restore re-erasure (STOR-D3 — per-subject
//!   re-erasure against the GDPR ledger) is **P-ST-14 (global P-100)**; this restore produces the
//!   prod-scale RESTORED copy online migrations rehearse lock-time against — **P-ST-21 (global
//!   P-126, STOR-D8)**; the real `pg_restore` + WAL-replay driver is the P-S12/P-S15 floor. The
//!   mutation floor on the cross-seam-point + referenced-hash-presence logic is ≥ 85%
//!   (mandatory-core — the silent-data-loss floor, the highest bar).
//! - **THE HEADLINE: the CI-wired restore-verify GATE (STOR-D1, the permanent gate — P-ST-13 / global
//!   P-061)** is now IMPLEMENTED in [`restore_verify`]: [`restore_verify::RestoreVerifyGate`] spins a
//!   clean target, drives `restore(to_offset T)` ([`restore`], P-060), and runs the three storage.md
//!   §7.4 assertions — (1) **no loss / checksum parity** (every restored row's referenced object is
//!   present AND its bytes re-hash to its BLAKE3 [`blob::ContentHash`] address; a present-but-corrupt
//!   object is the [`restore_verify::GateFailure::ChecksumMismatch`] the bare presence check misses,
//!   and a row → missing blob is the restore's hard §7.3 FAIL surfaced as
//!   [`restore_verify::GateFailure::RestoreFailed`]); (2) **cross-seam / one consistent point** (the
//!   harness `verify_cross_seam` assertion — the SAME SUB-D6 one, P-056 — reports 0 mismatches:
//!   derived == source-replay, no orphan, no past-offset); (3) **erasure held** (a tenant
//!   crypto-shredded BEFORE the backup stays erased — its KEK is excluded from the restored set, §7.5;
//!   a resurrected erased subject is [`restore_verify::GateFailure::ErasureResurrected`]). On PASS it
//!   emits a dated [`restore_verify::GreenArtifact`] with the MEASURED numbers; on RED a typed
//!   [`restore_verify::GateFailure`]. The verdict [`restore_verify::GateVerdict`] is `#[must_use]`
//!   (a dropped RED is a compile-flagged swallow) and the CI entrypoint
//!   [`restore_verify::RestoreVerifyGate::run_or_fail_ci`] turns a red into a process-failing `Err`
//!   (loud-never-swallowed, EI-01 §5 — no `|| true`). It REUSES [`restore`] (P-060), the harness
//!   cross-seam assertion (P-056), [`blob::ContentHash`] (P-047), and the KMS crypto-shred exclusion
//!   ([`kms`], P-058) — never re-defined; it ADDS the checksum-parity + erasure-held legs the bare
//!   restore lacks + the loud-never-swallowed CI gate. **This is one of the two permanent gates
//!   (master §4): it re-runs on every store-touching change, forever.** **Floors named in
//!   [`restore_verify`]:** post-restore RE-ERASURE (STOR-D3, per-subject re-erasure against the GDPR
//!   erasure ledger 10.8) + the cell-kill RTO half (STOR-D2) are the sibling **P-ST-14 (global
//!   P-100)** (this gate holds the erasure-BEFORE-the-backup invariant + exposes the
//!   [`restore_verify::ErasureLedger`] seam P-100 drives); the prod-scale restored copy for
//!   online-migration-under-load is **P-ST-21 (global P-126, STOR-D8)**; the real CI-runner wiring is
//!   M2+ (the gate runs as a `cargo test` drill until then); the real `pg_restore` driver is the
//!   P-S12/P-S15 floor. The mutation floor on the no-loss-assertion + the fail-CI-on-red branch is ≥
//!   85% (mandatory-core — the silent-data-loss floor, the highest bar).
//! - **Post-restore re-erasure (STOR-D3) + the cell-kill RTO drill (STOR-D2) (P-ST-14 / contract 11.5
//!   — global P-100)** is now IMPLEMENTED in [`reerase`]: it COMPLETES the headline. (1) **Post-restore
//!   re-erasure (§7.5 / GD-14):** [`reerase::ReErasePass::run`] re-applies every erasure the
//!   [`reerase::PostRestoreErasureLedger`] (10.8) records as completed AFTER the restore's PIT T — the
//!   set the restore could RESURRECT (a subject erased at offset `> T` still has a live pre-erasure DEK
//!   in the backup, which the before-the-backup gate leg P-061 does NOT cover). For each it RE-RUNS the
//!   P-099 [`erase::CryptoShredErase`] six-step algorithm (re-destroy the per-subject DEK + re-purge
//!   Search + re-tombstone Refs + re-emit `*.erased`) and asserts **0 resurrected subjects**
//!   ([`reerase::ReEraseReport::resurrected_count`] == 0). It is idempotent (the re-applied erase is
//!   itself a no-op success, P-099). **It is wired into the restore-verify gate**
//!   ([`restore_verify::RestoreVerifyGate::run_with_reerase`]) so every restore re-erases by
//!   construction — a resurrected post-T-erased subject FAILs the gate
//!   ([`restore_verify::GateFailure::ErasureResurrected`]). (2) **The cell-kill RTO drill (STOR-D2 RTO
//!   half, §7.1):** [`reerase::CellKillRestore`] models the begin-restore → consistent-ready wall-clock
//!   per grain ([`reerase::RtoGrain::Tenant`]/`Cell`); the drill asserts the measured RTO ≤ the
//!   `rpo_rto.rto_tenant_max_mins` (≤ 1 h) / `rto_cell_max_mins` (≤ 4 h) bound from the versioned
//!   `thresholds.toml` (never hardcoded), emitting onto the harness `RestoreRtoSecs{grain}` signal. It
//!   REUSES the P-099 [`erase::CryptoShredErase`] algorithm + its [`erase::EraseHolders`] seams, the
//!   [`restore::RestoreReport`] (P-060), the restore-verify gate + its before-the-backup
//!   [`restore_verify::ErasureLedger`] seam (P-061), the KMS crypto-shred exclusion ([`kms`], P-058),
//!   and the harness RTO model (P-056) — never re-defined; the NEW surface is the post-PIT ledger seam,
//!   the re-erasure pass, and the cell-kill RTO model. **Floors named in [`reerase`]:** the RTO numbers
//!   (≤ 1 h-tenant / ≤ 4 h-cell) are defaults-to-beat re-confirmed at cell scale in **P-ST-30 (M5)**;
//!   the §7.6 backup-window-vs-erasure-SLA residual number is `[OPEN → LEGAL]` (DPO-ratified — the
//!   MECHANISM ships, the NUMBER → counsel); the real GDPR erasure-ledger binding (10.8) is **P-GA-15
//!   (global P-115)**; the real `pg_restore` + cell-kill provisioning driver is the P-S12/P-S15 floor.
//!   The mutation floor on the post-PIT-select + re-apply + 0-resurrected-assert path is mandatory-core.
//! - **Residency pinning enforced end-to-end (STOR-D5) (P-ST-15 / contract 12.4 storage half + 12.1
//!   — global P-102)** is now IMPLEMENTED in [`residency`]: it CLOSES the per-pool runtime
//!   region-pin floor named in [`oltp`] + [`holder`]. (1) **The per-pool runtime region-pin:**
//!   [`residency::RegionPinnedStore`] pins every store to its cell's [`myelin_tenancy::Region`]
//!   (immutable — a region change is a NEW value); the M0 region-less-pool floor is closed. (2) **The
//!   in-process residency WRITE boundary:** [`residency::RegionPinnedStore::admit_write`] REJECTS a
//!   row whose region ≠ the store's pinned region ([`residency::ResidencyViolation::OutOfRegionWrite`])
//!   — *no store ever writes outside its region*, so cross-region replication has no source (the unit
//!   twin of the live-DB RLS `WITH CHECK` the STOR-D5 integration drill, P-096, proves against real
//!   Postgres). (3) **Every store reports its region:** [`residency::StoreResidencyReport`] is the
//!   per-store `(store_class, region)` report; (4) **the `myelin storage residency verify <tenant>`
//!   admin path** ([`residency::StoreSet::residency_verify`] → [`residency::verify_region_pinning`])
//!   gathers a report from EVERY M1 store class and FAILS LOUDLY on a cross-region store
//!   ([`residency::ResidencyViolation::OutOfRegionStore`]) or a missing one
//!   ([`residency::ResidencyViolation::MissingStoreReport`], fail-closed) — never a silent pass; on
//!   PASS it emits the PII-free [`residency::RegionPinningAttestation`] whose
//!   [`residency::ResidencyVerifySignal`] reads `cross_region_egress == 0` (the dated STOR-D5 green
//!   artifact). Storage is UPSTREAM of the control plane in the crate DAG, so it OWNS the
//!   report-producing side; the control plane's `residency_verify` (P-085) is the downstream CONSUMER
//!   that signs the reports — the 12.4 CDC pair (`tests/cdc_12_4_storage_residency_report.rs`) proves
//!   the two halves agree WITHOUT a shared report type (the DAG forbids a `myelin-storage ->
//!   myelin-control-plane` edge; documented deviation, EI-01 §1). **Floors named in [`residency`]:**
//!   the within-EU CDN edge set is **P-ST-23 (global P-254)**, the outbound push-mirror targets are
//!   **P-ST-25 (global P-255 — now LANDED, see [`mirror`])**, and the T3 firehose archive is
//!   **P-ST-20 (global P-147)** — all EXTEND this same `residency_verify` with additional store-class
//!   variants (the aggregation and fail-on-mismatch shape does not change). The mutation floor on the
//!   write-boundary region compare,
//!   the out-of-region-report branch, and the missing-store fail-closed branch is mandatory-core
//!   (≥ 80% — the region-pin enforcement carrying the token-region into the partition key).
//! - **The within-EU CDN clone/bundle blob class (C3) (P-ST-23 / contract 11.2-C3 + 12.4 — global
//!   P-254)** is now IMPLEMENTED in [`cdn`]: it FILLS the within-EU CDN edge-set floor [`residency`]
//!   named. (1) **A blob-class TAG over the unchanged [`blob::BlobStore`], NOT a new store (EI-01
//!   §7):** [`cdn::CdnCloneClass`] BORROWS the base content-addressed blob tier (`&dyn BlobStore`,
//!   never an owned second store) and serves clone/bundle blobs BY CONTENT-ADDRESS — a published
//!   bundle is an ordinary content-addressed blob ([`cdn::CdnCloneClass::publish_bundle`] →
//!   `BlobStore::put`), and a serve ([`cdn::CdnCloneClass::bundle`] → `BlobStore::get`)
//!   re-hash-verifies the bytes, so **the content-address IS the cache-validity check** (no staleness
//!   model to get wrong; a tampered bundle is refused — the STOR-D7 0-silent-serve floor rides
//!   through). The structural "not a new store" property is asserted by the SAME [`blob::FsBlobStore`]
//!   backing both a CDN serve and a plain trait `get`. (2) **Residency-respecting (within-EU edge
//!   set):** [`cdn::CdnEdgeSet::eligible_for`] filters a candidate POP set to ONLY the within-EU POPs
//!   for an EU tenant ([`cdn::CdnEdgePop::within_eu`]) — *no PII-bearing bundle reaches an extra-EU
//!   edge*; the within-EU classification is an INPUT (control-plane-sourced geography), Storage owns
//!   the FILTER (the mandatory-core). (3) **`residency_verify` covers the CDN edge set (extends
//!   P-ST-15, 12.4):** [`cdn::CdnCloneClass::residency_report`] produces a
//!   [`residency::ResidencyStoreClass::CdnEdgeSet`] report @ the tenant's region, fed into the SAME
//!   [`residency::verify_region_pinning`] aggregation — a CDN serving an EU tenant from an extra-EU
//!   region FAILs the attestation WITHOUT a code change (the aggregation already checks any reported
//!   class's region; STOR-D5 CDN extension → 0 cross-region PII egress via the CDN). It REUSES
//!   [`blob`] (P-047) + [`residency`] (P-102) — never a parallel store or a second residency shape.
//!   **Floor named in [`cdn`]:** the C6 outbound push-mirror residency gate (the mirror TARGET added
//!   to the same attestation) is the sibling **P-ST-25 (global P-255)**; the real edge-delivery POP
//!   fleet + the object-store backing the bundles ultimately rest on are deployment/M5 follow-ons
//!   (P-ST-30/P-ST-31) — a backing swap by the trait's design. The mutation floor on the
//!   within-EU-edge-set filter + the content-address validity check is mandatory-core (≥ 80%).
//! - **The outbound push-mirror residency gate SEAM (C6) (P-ST-25 / contract 10.5 consumed + 12.4 —
//!   global P-255)** is now IMPLEMENTED in [`mirror`]: it FILLS the C6 outbound-push-mirror floor
//!   [`residency`] + [`cdn`] named. Storage FLAGS the crossing — it does NOT author the allow/deny
//!   (the GATE is GDPR `transfer_allowed` 10.5 + the control plane's `mirror_allowed` deny-by-default,
//!   P-251). (1) **Mirror-source blobs content-addressed + encrypted (storage.md §6(a)):**
//!   [`mirror::PushMirrorClass`] BORROWS the tenant's content-addressed blob tier (`&dyn BlobStore`,
//!   never a new store) — the mirror-source bytes are ordinary content-addressed blobs sealed under
//!   the per-tenant blob DEK ([`encryption::DekContentWrap`], the same seam the git pack tier uses);
//!   [`mirror::PushMirrorClass::source_is_content_addressed_and_encrypted`] proves the address is the
//!   plaintext BLAKE3 + a tampered source blob is refused (STOR-D7 rides through). (2) **The C6 flag
//!   into `residency_verify` (storage.md §6(b), 12.4):** [`mirror::PushMirrorClass::residency_report`]
//!   reports [`residency::ResidencyStoreClass::PushMirror`] @ **the mirror TARGET's region** into the
//!   SAME [`residency::verify_region_pinning`] aggregation — an extra-EU mirror target FAILs the
//!   attestation WITHOUT a code change (the no-extra-EU-PII property is attestable; a same-region
//!   mirror reports the tenant's own region — no crossing). (3) **The `mirror_residency_deny{tenant}`
//!   telemetry (C6 / D-S13):** [`mirror::MirrorTelemetry::flag_crossing`] counts the flagged
//!   extra-region crossings the control-plane gate denies — *0 PII reaches an ungated extra-EU mirror*
//!   (the byte never leaves: the gate denies, the flag makes the crossing attestable). The class
//!   exposes NO `allow`/`deny` (the structural ownership split — Storage flags, the control plane
//!   gates). It REUSES [`blob`] (P-047) + [`residency`] (P-102) + the control-plane `mirror_allowed`
//!   gate (P-251) — never a parallel store or a second mirror policy (EI-01 §7); the CDC pair
//!   (`tests/cdc_10_5_mirror_crossing_flag.rs`) proves Storage's flag REACHES the control-plane gate.
//!   **Floors named in [`mirror`]:** the `transfer_allowed` lawful-basis entries for a SPECIFIC
//!   extra-EU mirror are `[OPEN — LEGAL]` (Schrems II — counsel/DPO ratifies; the engineering gate
//!   denies by default regardless); the real `git push --mirror` transport is the Git subsystem M3
//!   consumer (it consults the gate before pushing; here the storage flag is complete + proven). The
//!   mutation floor on the report-the-TARGET-region flag + the `mirror_residency_deny` increment + the
//!   content-address property is mandatory-core (≥ 80%).

pub mod backup;
pub mod blob;
// The object-store BlobStore's replica-recovery read path (P-ST-30 / global P-441): the STOR-D7
// "recover from a replica" property added at the object tier, backing-agnostic over the
// unchanged BlobStore trait (the fs floor in CI, the live S3BlobStore in the integration test).
pub mod replicated_blob;
// The within-EU CDN clone/bundle blob class (C3, P-ST-23 / P-254, contract 11.2-C3 + 12.4): a
// content-addressed blob CLASS over the UNCHANGED `BlobStore` trait (a tag + an eligible-edge-set
// policy, NOT a new store — EI-01 §7) for hot-repo/clone-storm acceleration. The content-address
// IS the cache-validity check (no staleness model). Residency-respecting: an EU tenant's eligible
// edge set is within-EU-only (the `CdnEdgeSet` filter, mandatory-core). EXTENDS `residency_verify`
// with the `CdnEdgeSet` store class (the CDN report feeds the SAME `verify_region_pinning`
// aggregation — a cross-region CDN edge FAILs there, 0 cross-region PII egress). FLOOR NAMED: the
// C6 outbound push-mirror target is the sibling P-ST-25 / P-255. REUSES blob (P-047) + residency
// (P-102) — never a parallel store or a second residency shape.
pub mod cdn;
// The trust-scoped CI cache namespaces (C4, P-ST-28 / P-330, contract 11.2-C4): the scope-key
// convention `<tenant>/ci/cache/<scope>/...` (`<scope> ∈ {trusted, fork:<pr_id>, branch:<name>}`)
// over the UNCHANGED `BlobStore` trait (a namespace + a write-scope refusal, NOT a new store —
// EI-01 §7). An `untrusted_fork` run may READ the `trusted` scope (cache hits are fine) but its
// WRITE to `trusted` is REFUSED by the blob client (the poisoned-cache defence, the storage half of
// X-1). The scope is stamped from the CI-stamped `trust_tier` (an INPUT — Storage ENFORCES, it does
// NOT recompute trust). Wires `cache_scope_violation{tenant}` (D-S11, must be 0). SIBLING of the
// GIT-side `fork_gate::ScopedCache` (T7 coordination cache) at the T2 BLOB tier — distinct, not a
// duplicate. FLOOR NAMED: the C5 OLAP restriction-flag gate is the sibling P-ST-29 / P-331. REUSES
// blob (P-047) — never a parallel store.
pub mod ci_cache_scope;
// The local-disk git pack/object tier behind the BlobStore trait (P-ST-22 / P-252, contract 11.2
// git pack tier + 12.2 repo-granular relocatable placement): git packs + loose objects are
// addressed THROUGH the content-addressed `BlobStore` trait (REUSES blob.rs — never a parallel
// store), so "local-disk → object-store-backed packs" is a backing SWAP not a rewrite (§3.5). A
// repo's placement is region-pinned + node-RELOCATABLE (a stored fact, no node hash) — the
// relocatability §3.5 DECIDES now. SHA-256 read-side verify (closing the blob.rs P-ST-22 floor)
// detects a corrupt git object on read (0 silent serve, STOR-D7 on packs); recovery is re-fetch
// of the same content address from a replica. FLOORS NAMED: object-backed pack/delta + smart
// transport → M5 P-ST-31 (a backing swap by the trait's design, trigger GIT-D4); the within-EU CDN
// clone/bundle class (C3) → sibling P-ST-23.
pub mod git_shred;
pub mod gitpack;
// Object-backed git packs — the local-disk-packs follow-on (P-ST-31 / P-442, contract 11.2, EI-04
// §3): authoritative git bytes move from node-local disk (P-ST-22 `gitpack`) onto the OBJECT tier
// (P-ST-30 `replicated_blob` over the object store). Because git packs are addressed THROUGH the
// `BlobStore` trait (the §3.5 seam DECIDED at M3), the move is a backing SWAP, not a rewrite — the
// consumer (`GitPackTier`) is byte-for-byte untouched. This module makes the transition a NAMED,
// testable thing: `object_backed_pack_tier` builds a `GitPackTier<ReplicatedBlobStore<B>>`; the C3
// CDN clone class (P-ST-23) is wired against the object backing; the GIT-D4 ceiling gate measures
// the single-node clone-serve ceiling crossing (the trigger, §8 measure-before-shard) + proves the
// object-backed packs serve clone p99 within budget; STOR-D7 stays green (re-hash-on-read + recover-
// from-replica carry to the object-backed packs). REUSES gitpack + replicated_blob + cdn — never a
// parallel store or a second tier (EI-01 §7). FLOOR PROMOTED: the local-disk-packs floor is now its
// full answer. NAMED-not-built: the object-backed pack-ALGORITHM impl (chunking/delta-base/smart-
// transport) is co-owned with the Git subsystem M5 deliverable (GIT-P33).
pub mod object_packs;
// The outbound push-mirror residency gate SEAM (C6, P-ST-25 / P-255, contract 10.5 consumed + 12.4):
// Storage FLAGS the residency-boundary crossing of a Git push-mirror — it (a) keeps mirror-source
// blobs content-addressed + encrypted (REUSES blob + DekContentWrap — never a new store), and (b)
// REPORTS the mirror TARGET region into `residency_verify` (the `PushMirror` store class feeds the
// SAME `verify_region_pinning` aggregation, so an extra-EU target FAILs the attestation WITHOUT a
// code change). The `mirror_residency_deny{tenant}` telemetry counts flagged extra-region crossings
// (0 PII to an ungated extra-EU mirror, D-S13). Storage authors NO allow/deny — the GATE lives at
// GDPR `transfer_allowed` (10.5) + the control plane's `mirror_allowed` (deny-by-default, P-251);
// the CDC pair proves Storage's flag reaches that gate. REUSES blob (P-047) + residency (P-102) +
// the control-plane gate (P-251) — never a parallel store or a second mirror policy (EI-01 §7).
pub mod encryption;
pub mod mirror;
// The T3 firehose-archive seam (P-ST-20 / P-147, M2, contract 11.8 sealing + per-tenant-DEK half):
// the DURABLE archive of the firehose (storage.md §3.3). It RIDES the 3.5 resume-cursor transport
// (`myelin_events::Firehose`, P-141) — consuming the SAME `Frame`s a live viewer subscribes to —
// and SEALS frame batches into content-addressed T2 blobs encrypted under the per-tenant DEK
// (inheriting T2 crypto-shred): a destroyed tenant DEK renders the segment unrecoverable, live AND
// in backups by construction. Telemetry: `unencrypted_segment_count == 0`,
// `segment_content_addressed == true`. It REUSES blob (P-047) + DekContentWrap/KmsEngine
// (P-095/P-058) — never a parallel firehose or key store — and EXTENDS `residency_verify` with the
// `T3FirehoseArchive` store class. Validated on a NON-CI firehose (a synthetic op-stream). FLOORS
// NAMED: the CI `(job,step,byte-range)` index (C2) is P-ST-26 (M4); the per-subject CI-log DEK (C1)
// is P-ST-27 (M4) — a key-class swap on the same DekContentWrap seam.
pub mod erase;
pub mod firehose_archive;
// The T3 CI log tier (C2, P-ST-26 / P-328, M4, contract 11.8 the (job,step,byte-range) index +
// consumed 5.9 the CheckStatus.details_ref #step-<n> resolution): the CI-keyed instance of the
// P-ST-20 FirehoseArchiver (the SEALING + per-tenant-DEK mechanism — REUSED wholesale, never a
// second seal path). It adds the ONE M4 follow-on P-ST-20 named: the `(job, step, byte-range)`
// index. A CI log frame carries `(job, step, chunk-bytes)`; sealing a batch flushes a
// content-addressed T2 segment (inheriting T2 encryption + crypto-shred) AND records, per
// `(job, step)`, the byte-range that chunk occupies in the reconstructed step log — the
// `(job, step, byte-range) → (segment-blob, offset)` resolver. `resolve_step_anchor` parses the
// X-1 `myelin://.../ci/run/<id>#step-<n>` jump-to-failure sub-anchor (5.9 / OQ-D) and reads ONLY
// the indexed segment(s), slicing out EXACTLY the failing step's bytes. A `#step-<n>` for a step
// the index never saw is a LOUD CiLogError::UnknownStep. REUSES firehose_archive (P-147) +
// residency's T3FirehoseArchive class (a CI log segment IS a T3 firehose segment — no new
// variant). FLOORS NAMED: the per-SUBJECT CI-log DEK (C1) is the sibling P-ST-27 (a key-class swap
// on the same DekContentWrap seam); the real OLTP `ci_log_index` table (UNIQUE(job,step,seq)) is
// the P-S12/P-S15 backing swap (the in-process map is the index SHAPE).
pub mod ci_log_index;
pub mod gd4;
// The minimal cache seam (Stage 1 / infra — NEW). No cache trait existed before; this is the
// one-line-swap Cache trait (in-memory floor + Valkey/Redis backing behind `integration`).
pub mod bus_shred;
pub mod cache;
// The STORAGE half of the cross-cell PII-free pointer bridge + cell→cell migration (CP-D7, P-ST-32 /
// P-443, M5): restore the source's §7.3 cross-seam consistency point INTO the target cell (0 loss) +
// crypto-shred the SOURCE cell's key (source unrecoverable); the cross-cell pointer carries only an
// opaque subject (resolution cell-local, no PII crosses). Composes restore.rs + kms.rs; the
// control-plane half (durable workflow + atomic cut-over) is myelin_control_plane::migration.
pub mod cell_migration;
pub mod coloc;
pub mod holder;
pub mod key_origin;
pub mod kms;
pub mod kms_failstatic;
pub mod migration;
// The STOR-D8 online-migration-under-load drill (P-ST-21 / P-126, M2): expand→backfill→contract on
// the restored prod-scale copy under load, lock-wait p99 within budget + 0 downtime. Reuses the
// online runner (migration.rs) for admission + the restore-verify gate's restored copy.
pub mod migration_under_load;
// The OLAP read store FRAME — the holder + the CQRS-fed-by-the-bus contract shape (P-ST-17 /
// P-104, contract 11.6 partial): a per-cell residency-pinned, idempotent-consumer-fed (dedup on
// `event_id`) analytics read model, populated ONLY by replaying the durable event stream — live
// (`OlapReadStore::apply`) or cold (`OlapReadStore::reindex_from_source`), NEVER by scanning OLTP
// (the structural guard `oltp_scan_path_count == 0` — reindex-from-source is the ONLY rebuild path,
// no "read OLTP into ClickHouse" backdoor). The OLAP store registers as a `PersonalDataHolder`
// (`OlapStoreHolder`, crypto-shred erasure). FLOORS NAMED: the live bus feed (steady state) is
// P-ST-18 (P-145); the C5 restriction-flag analytics-suppression gate lights up with Issues
// analytics in M4 (P-ST-29) — the frame carries the flag; the worklog analytics-eligibility is
// [OPEN → LEGAL] (OQ-H). See the module-level DEVIATION note (OLAP stays out of the frozen
// residency M1 backup-able set because T4 is a derived, NOT-backed-up, reindex-from-source store).
pub mod olap;
pub mod olap_feed;
// The OLAP restriction-flag gate (C5, P-ST-29 / P-331, M4, contract 11.6 the C5 gate + consumed
// 10.1 `restrict(subject)`): `restrict(subject)` propagates into T4 — a restricted subject's rows
// are EXCLUDED from every analytics aggregate (CFD/cycle-time/velocity/delivery-health). The OLAP
// consumer applies the restriction flag as a QUERY-TIME filter (`OlapAnalytics` over the frame's
// read model — never a second store, EI-01 §7); the subject's contribution is withheld until
// restriction lifts (it reappears with no reindex — the rows stay) or erasure completes. A
// COMPLIANCE gate, not a tuning knob — it unblocks the Issues analytics ask without leaking a
// restricted subject. Wires `olap_restricted_subject_leak` (D-S12, must be 0 — the leak audit
// intersects each aggregate's contributing-subject set with the restriction set). REUSES olap.rs's
// `OlapReadStore` docs + restriction set (P-104/P-145) — never a parallel store. FLOOR NAMED: the
// worklog/productivity/estimate analytics-ELIGIBILITY (OQ-H) is [OPEN — LEGAL] (works-council
// consultation) — Storage ships the `AnalyticsEligibility` SEAM (conservative default: per-individual
// rollups OFF) + the C5 restriction gate REGARDLESS; the LEGAL ratification of which rollups are
// eligible is the named follow-on. STOR-D1/STOR-D2 remain green (no restore/backup code touched).
pub mod olap_restrict;
pub mod oltp;
pub mod reerase;
// The reserve/settle cost gate mechanism + the durable per-tenant ledger (P-ST-16 / P-103,
// contract 11.7): reserve-at-dispatch (no balance → no run), settle-on-completion (one cost
// event per metered unit, wholesale ≠ markup recorded distinctly), NEVER interrupt in-flight
// (the counter is 0 by construction — no code path increments it), integer minor-units (a
// float cost is unrepresentable). Storage owns the durable ledger correctness; the gate
// FRONTS agent runs in M2 (P-ST-19 / P-146) and CI runs in M4 (named floors).
pub mod reserve_settle;
// Reserve/settle FRONTS agent runs — the live consumer half of 11.7 (P-ST-19 / P-146): the
// dispatch-fronting gate that now sits in front of every `AgentRuntime` run + every
// `SCHEDULE_AND_RUN_JOB`. Reserve-at-dispatch → no balance → no run (no in-flight handle is
// minted); the run executes behind a move-only `InFlightRun` handle whose ONLY exit is
// settle-on-completion; the gate exposes NO API that interrupts an in-flight run (never
// interrupt in-flight is structural). Drives the P-ST-16 `CostLedger` (Storage owns the
// durable ledger correctness). Fills the P-ST-16 floor; the CI-run-fronting M4 follow-on is
// named. Drill rows AG-D6 (surge sheds over-budget) / AG-D11 (runaway loop stops at the wallet).
pub mod agent_run_gate;
// Residency pinning enforced end-to-end (STOR-D5, P-ST-15 / P-102): the per-pool runtime region-pin
// (closes the oltp/holder M0 floor), the in-process residency WRITE boundary, the per-store region
// report, and the `myelin storage residency verify <tenant>` admin path. The control plane's
// `residency_verify` (P-085) consumes the reports this produces (the 12.4 CDC; storage is upstream
// of the control plane in the DAG, so it OWNS the report-producing side).
pub mod residency;
pub mod restore;
pub mod restore_verify;
pub mod rls;

// ---- Stage 2 / infra: the REAL backends behind the existing traits (config-selected) ----
// These modules are compiled ONLY under `--features integration` (they pull the real sqlx /
// aws-sdk-s3 / fred clients). The default `cargo build --workspace` compiles NONE of them, so
// it stays DB-free. Each implements an EXISTING trait — it does not fork one:
//   - s3blob::S3BlobStore  implements blob::BlobStore  (object store, RustFS/Scaleway)
//   - valkey::ValkeyCache  implements cache::Cache     (Valkey/Redis)
//   - pg::PgStore          backs the OLTP + outbox/relay + ReBAC tuple store on real Postgres
// The `backend` module is the config-selection seam (real-vs-in-memory from MyelinConfig).
#[cfg(feature = "integration")]
pub mod pg;
// The race-safe LIVE migration DRIVER (the P-S12 floor): a forward-only, idempotent, SERIALIZED
// (Postgres session advisory lock on a fixed app-wide key), version-recorded migrator. It fixes the
// concurrent-`CREATE TABLE` `pg_type_typname_nsp_index` race that the bare
// `raw_sql(ddl).execute(&pool)` sites (PgStore::migrate, git check_status) had. Behind `integration`
// like the rest of the live-PG code.
#[cfg(feature = "integration")]
pub mod pg_migrator;
#[cfg(feature = "integration")]
pub mod s3blob;
#[cfg(feature = "integration")]
pub mod valkey;
// The OLTP-co-located outbox relay (the one legitimate broker-publish site, BUS-2) — kept in its
// own module so the broker-publish call is isolated to a single named relay file (the same
// posture as myelin-events/src/relay.rs).
#[cfg(feature = "integration")]
pub mod backend;
#[cfg(feature = "integration")]
pub mod pgrelay;

pub use agent_run_gate::{AgentRunGate, AgentRunGateSignal, DispatchError, InFlightRun, RunKind};
pub use backup::{
    BackupError, BackupSet, BaseBackup, ContinuousArchiver, EpochSecs, LogTierSeal,
    ObjectTierBackup, ObjectVersion, StoreTier, WalOffset, WalSegment,
};
pub use blob::{
    BlobError, BlobMeta, BlobStore, BlobTelemetry, ContentHash, ContentWrap, FsBlobStore, HashAlgo,
    IdentityWrap,
};
pub use bus_shred::KmsBusShredder;
pub use cache::{Cache, CacheError, InMemoryCache};
pub use cdn::{CdnCloneClass, CdnEdgePop, CdnEdgeSet};
pub use cell_migration::{
    is_cell_local, migrate_cell_to_cell, storage_resolves_locally, CellMigrationError,
    CellMigrationReceipt, CellMigrationRequest, CellTenantTiers,
};
pub use ci_cache_scope::{
    CacheScope, CacheScopeError, CacheScopeTelemetry, CiCacheNamespace, TrustTier,
};
pub use ci_log_index::{
    CiLogError, CiLogFrame, CiLogIndex, CiLogTier, SegmentKeying, StepAnchor, StepSpan,
    CI_LOG_STREAM,
};
pub use coloc::{ColocError, ColocatedOltp, ColocatedTx, COLOCATED_OUTBOX_MIGRATION};
pub use encryption::{
    key_class_for, ColumnCryptor, DekContentWrap, EncryptedColumn, KeyChoiceError, SubjectId,
};
pub use erase::{
    BlobShredReach, BusErase, CryptoShredErase, EpochMillis, EraseError, EraseHolders,
    ErasureLedgerSink, ErasureReceipt, PseudonymShred, RefsTombstone, SearchPurge,
};
pub use firehose_archive::{
    segment_pointer_draft, ArchiveError, ArchiveTelemetry, FirehoseArchiver, SealedSegment,
    SegmentBytes,
};
pub use gd4::{
    assert_gd4_table_complete, assert_no_local_residual_statement, granularity_of_key_class,
    key_choice_granularity, structural_reach_uses_erase_seams, DataClass, Gd4TableReport,
    KeyGranularity, StructuralErasureFloor, StructuralFloorReport, RESIDUAL_POSTURE_REF,
};
pub use git_shred::{GitCryptoShredReach, GitResidual, GitShredReceipt, GitShreddable};
pub use gitpack::{
    git_object_address, GitObjectKind, GitPackError, GitPackTier, PackManifest, PlacementError,
    RepoGitPlacement, RepoId, RepoPlacementStatus, StorageGroup,
};
pub use holder::{register_holder, BlobStoreHolder, OltpHolderRegistration, OltpStoreHolder};
pub use key_origin::{
    Byok, Dek, Hyok, HyokKeyService, HyokServiceDenied, IndexAdmission, KeyId, KeyOrigin,
    KeyOriginError, KeyOriginKind, KeyOriginTelemetry, PlatformManaged,
};
pub use kms::{
    CellRoot, DekHandle, DekId, KekId, KeyClass, KmsAdapter, KmsEngine, KmsError, PiiKeyRef,
    WrappedDek, KEY_LEN, NONCE_LEN,
};
pub use kms_failstatic::{
    KmsFailStaticSignals, KmsReadError, KmsReadPath, KmsReadResult, KmsReadiness,
};
pub use migration::{
    is_blocking_alter, is_destructive, HotTables, Migration, MigrationError, MigrationPhase,
    Migrations, OnlineMigrationRunner, PhaseProgress,
};
pub use migration_under_load::{
    lock_cost_ms, LockBudget, LockClass, MigrationLoadArtifact, MigrationLoadFailure,
    MigrationLoadVerdict, MigrationUnderLoad, StepLockMeasure, WriteLoad,
};
pub use mirror::{MirrorTelemetry, PushMirrorClass, PushMirrorTarget};
pub use object_packs::{
    cdn_over_object_backing, object_backed_pack_tier, place_repo_object_backed,
    served_from_object_tier, CloneStormLoad, GitD4Ceiling, GitD4Report, ObjectBackedServe,
    SingleNodeServe,
};
pub use olap::{
    OlapApply, OlapDoc, OlapEvent, OlapFrameSignal, OlapIngestError, OlapReadStore, OlapStoreHolder,
};
pub use olap_feed::{
    reindex_olap_from_bus, OlapAnalyticsSource, OlapBusConsumer, OlapReindexParitySignal,
};
pub use olap_restrict::{
    AnalyticsAggregate, AnalyticsEligibility, OlapAnalytics, RestrictionGateSignal,
    RestrictionLeakAudit,
};
pub use oltp::{OltpConfig, OltpError, OltpPool, PermitGuard};
pub use reerase::{
    CellKillRestore, CellKillRtoReport, ErasureRecord, InMemoryPostPitLedger,
    PostRestoreErasureLedger, ReErasePass, ReEraseReport, ReErasedSubject, RtoGrain,
};
pub use replicated_blob::{ReplicaTelemetry, ReplicatedBlobStore};
pub use reserve_settle::{
    CostEvent, CostLedger, MeteredUnit, MinorUnits, Reservation, ReservationState, ReserveError,
    ReserveSettleSignal, RunId, SettleError, SettleOutcome,
};
pub use residency::{
    verify_region_pinning, RegionPinnedStore, RegionPinningAttestation, ResidencyStoreClass,
    ResidencyVerifySignal, ResidencyViolation, StoreResidencyReport, StoreSet,
};
pub use restore::{
    restore_to_offset, restored_key_counts, BlobPresence, ReindexFromSource, RestoreError,
    RestoreReport, SourceEvent, SourceLog, WalRow,
};
pub use restore_verify::{
    ErasureLedger, GateFailure, GateInputs, GateVerdict, GreenArtifact, RestoreTarget,
    RestoreVerifyGate, RestoredObject,
};
pub use rls::{RlsError, TenantQuery, TenantScope, TenantTable};

// The race-safe live migration driver (the P-S12 floor) — re-exported behind `integration`, along
// with the live `PgStore` + its typed `PgError` the driver returns.
#[cfg(feature = "integration")]
pub use pg::{PgError, PgStore};
#[cfg(feature = "integration")]
pub use pg_migrator::{with_migration_lock, PgMigrator, MIGRATION_LOCK_KEY};

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
//!   `can_derive_plaintext_index()=false` structural HYOK enforcement) is the SIBLING **P-ST-07
//!   (global P-094)** that FRONTS this engine; the OLTP/blob ENCRYPTION wiring (classify-driven
//!   key choice, the real per-blob content-key wrap) is **P-ST-08 (global P-095)**; the
//!   per-content-class HYOK POLICY + the KMIP/external-key-store adapter + HYOK-as-Schrems-III
//!   (GD-7) are `[OPEN → P6/LEGAL]` named follow-ons (mechanism ships; policy → counsel/DPO); the
//!   HSM/Shamir-split L0 backing is the production-hardening follow-on (the SHAPE — root wraps
//!   KEKs, never exported — is complete). The mutation floor on the wrap/unwrap/destroy +
//!   fail-open-prevention path is ≥ 80% (mandatory-core).
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

pub mod backup;
pub mod blob;
pub mod coloc;
pub mod holder;
pub mod kms;
pub mod kms_failstatic;
pub mod migration;
pub mod oltp;
pub mod restore;
pub mod rls;

pub use backup::{
    BackupError, BackupSet, BaseBackup, ContinuousArchiver, EpochSecs, LogTierSeal, ObjectTierBackup,
    ObjectVersion, StoreTier, WalOffset, WalSegment,
};
pub use blob::{
    BlobError, BlobMeta, BlobStore, BlobTelemetry, ContentHash, ContentWrap, FsBlobStore,
    HashAlgo, IdentityWrap,
};
pub use coloc::{ColocError, ColocatedOltp, ColocatedTx, COLOCATED_OUTBOX_MIGRATION};
pub use holder::{register_holder, BlobStoreHolder, OltpHolderRegistration, OltpStoreHolder};
pub use kms::{
    CellRoot, DekHandle, DekId, KeyClass, KekId, KmsAdapter, KmsEngine, KmsError, PiiKeyRef,
    WrappedDek, KEY_LEN, NONCE_LEN,
};
pub use kms_failstatic::{
    KmsFailStaticSignals, KmsReadError, KmsReadPath, KmsReadResult, KmsReadiness,
};
pub use migration::{
    is_blocking_alter, is_destructive, HotTables, Migration, MigrationError, MigrationPhase,
    Migrations, OnlineMigrationRunner, PhaseProgress,
};
pub use oltp::{OltpConfig, OltpError, OltpPool, PermitGuard};
pub use restore::{
    restore_to_offset, restored_key_counts, BlobPresence, ReindexFromSource, RestoreError,
    RestoreReport, SourceEvent, SourceLog, WalRow,
};
pub use rls::{RlsError, TenantQuery, TenantScope, TenantTable};

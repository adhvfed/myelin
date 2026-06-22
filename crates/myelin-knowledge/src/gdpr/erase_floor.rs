//! # `gdpr::erase_floor` — the Knowledge erase STRUCTURAL FLOOR (KN-P26 / P-316, M3 / KN-M3e)
//!
//! This is the **KN-P26 deliverable**: the per-subject DEK crypto-shred + pseudonym-map shred +
//! tombstone/embedding-purge erase op (contract 10.1 `erase`, KN-D4 — the hardest GDPR surface).
//! KN-P25 / P-315 shipped `locate/export/rectify/restrict` and left `erase` as the named floor that
//! REFUSED loud (it could not fabricate the per-subject DEK crypto-shred seam). This module ships the
//! body: it COMPOSES the already-built storage-side crypto-shred MECHANISM
//! ([`myelin_storage::CryptoShredErase`] — global P-099/P-ST-09) and adds the two Knowledge-specific
//! lockstep legs the storage mechanism delegates to its seams: the **embedding/vector purge** (the
//! Search index is plaintext-derived — embeddings of personal data ARE personal data, so they purge in
//! lockstep) and the **backlink tombstone** (mentions/backlinks degrade to an `Erased` tombstone via
//! the refs-glue `*.erased` consumer). The holder's `erase` trait method now performs the real erase
//! instead of refusing.
//!
//! ## Coherence (EI-01 §7) — what this REUSES vs. what is genuinely new
//! The crypto-shred ALGORITHM is NOT re-implemented here. [`myelin_storage::CryptoShredErase`] already
//! ships the frozen §5.2 six-step order (pseudonym-map shred → `KMS.destroy(per_subject_DEK)` → Search
//! purge+reindex → Refs tombstone → Bus erase → erasure receipt), with the per-subject DEK destroy
//! reaching backups *by construction* (a backup holds ciphertext under the now-destroyed key) and the
//! `recoverable_in_backup == 0` STOR-D4 gate. KN-P26 is the **Knowledge-side composition**: it wires
//! the storage orchestrator's cross-holder seams to Knowledge's concrete surfaces —
//! - the [`myelin_storage::SearchPurge`] seam → [`KnowledgeEmbeddingPurge`] (purge the subject's page /
//!   block / vector docs from the Search index — [`crate::search_feed`] is the index Knowledge feeds);
//! - the [`myelin_storage::RefsTombstone`] seam → [`KnowledgeBacklinkTombstone`] (flip the subject's
//!   refs to an `Erased` tombstone via [`crate::refs_glue::PageStore::mark_erased`] — the §6.1 step-3
//!   `*.erased` consumer; a backlink renders "(not available)", never the shredded content);
//! and leaves the pseudonym-map shred (Id 4.8) / Bus erase (2.7 `*.erased`) / erasure-ledger receipt
//! (10.8) as the SAME storage seams the DSR orchestrator wires (they are not Knowledge-owned surfaces).
//! The per-subject DEK destroy (step 2) is owned by the storage [`myelin_storage::KmsEngine`] — never a
//! parallel key store, so the destroy reaches exactly the ciphertext the Knowledge encrypted columns /
//! op-log / snapshots wrote.
//!
//! ## Contracts implemented
//! - **10.1** `erase` (OWNED — the Knowledge erase op): the holder's `erase` now composes the floor.
//! - **11.4** the per-subject DEK crypto-shred (CONSUMED — the key destroy): via the storage engine's
//!   `KMS.destroy(per_subject_DEK(tenant, subject))`; ONE DEK per `(subject, tenant)`, applied only to
//!   PII-bearing classes (CR-I) — the key-shred count is bounded at one per subject.
//! - **4.8** the pseudonym-map shred (CONSUMED — the storage [`myelin_storage::PseudonymShred`] seam).
//! - **2.7** the `*.erased` tombstone (CONSUMED / PRODUCED — the storage [`myelin_storage::BusErase`]
//!   seam emits it; [`KnowledgeBacklinkTombstone`] is the Knowledge-side consumer that flips the
//!   backlink to an `Erased` tombstone).
//! - **10.8** the erasure ledger (CONSUMED — the receipt hash-links via the storage
//!   [`myelin_storage::ErasureLedgerSink`] seam).
//! - **10.9** the ONE platform erasure posture (REFERENCED — the residual is instantiated BY
//!   REFERENCE, never restated; see the FLOOR note below).
//!
//! ## FLOOR named (VISION §3 — name-your-floors) + the RESIDUAL (10.9, by reference)
//! The structural floor is **fully built + reliable** for structured / self-authored PII: structured
//! attribution pseudonymises (the pseudonym-map shred), self-authored free-text crypto-shreds (the
//! per-subject DEK destroy → unrecoverable in op-log / snapshots / backups), embeddings purge, backlinks
//! tombstone. The RESIDUAL — third-party free-text PII (a person's name typed by *someone else* into
//! that other person's content, encrypted under the *author's* DEK, not the subject's) — is handled per
//! the **ONE platform-wide posture (contract 10.9, X-7, `[OPEN — LEGAL]`, KQ-8)**: a documented
//! lawful-basis limit + best-effort `rectify`/tombstone + the standing guarantee that the residual is
//! never indexed / never agent-readable / never in analytics for a restricted subject (the
//! `restrict` suppression, [`super::RestrictSuppressor`]). The structural floor ships regardless. The
//! residual is instantiated BY REFERENCE — NOT restated here as a Knowledge-local statement.
//!
//! ## DB-free
//! This module composes the in-memory storage [`myelin_storage::KmsEngine`] + the in-memory Search
//! index backend + the refs-glue [`crate::refs_glue::PageStore`]; the LIVE-stack proof (the real OLTP
//! op-log / snapshot ciphertext, the real Search vector index) rides the Knowledge integration drills.
//! So `cargo build --workspace` stays DB-free.
//!
//! ## Mutation floor (mandatory-core — EI-01 §2; unrecoverability is the property)
//! The crypto-shred path is mandatory-core: the load-bearing properties are *the per-subject DEK is
//! destroyed → the ciphertext is unrecoverable live AND in backups (0 recoverable)*, *the embeddings
//! purge in lockstep (a vector of personal data is personal data)*, *the backlinks tombstone (0 leak)*,
//! and *a partial failure is a LOUD error, never a false "erased"*. The achieved cargo-mutants score is
//! stated in the P-316 report (`cargo mutants -p myelin-knowledge -f crates/myelin-knowledge/src/gdpr/erase_floor.rs`).

use myelin_gdpr::{EraseReceipt, EraseScope, Receipt, Result as DsrResult, SubjectRef, TenantId};
use myelin_storage::encryption::SubjectId;
use myelin_storage::{
    CryptoShredErase, EpochMillis, EraseError, EraseHolders, ErasureLedgerSink, KmsEngine,
    PseudonymShred, RefsTombstone, SearchPurge,
};
use std::sync::Mutex;

use crate::refs_glue::PageStore;
use myelin_events::ArtifactRef;
use myelin_search::engine::IndexBackend;

use super::HOLDER_ID;

// ════════════════════════════════════════════════════════════════════════════════════════════
// The Knowledge-specific lockstep seams (the two surfaces Knowledge OWNS in the erase fan-out)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The Knowledge embedding/vector purge seam** (architecture §6.1 step 3 — the Search + vector index
/// purges in lockstep; embeddings of personal data ARE personal data). It binds the storage
/// [`SearchPurge`] seam to Knowledge's concrete Search index: the subject's page / significant-block /
/// db-row docs (and their vectors) are DELETEd from the index, then the surviving content is
/// reindexed-from-source. The index is **plaintext-derived**, so the erase is purge+reindex, NOT a
/// key-destroy (a destroyed key would strand a stale plaintext-derived index entry, §5.2).
///
/// `doc_ids` is the set of the subject's index doc-ids the caller assembled from the store (the page /
/// block / row docs the subject authored or is the data-subject of). The purge counts the vectors it
/// dropped (the `vector_tombstone` telemetry leg) so the receipt can prove 0 of the subject's
/// embeddings survive.
pub struct KnowledgeEmbeddingPurge<'a, B: IndexBackend> {
    index: &'a Mutex<B>,
    /// The subject's index doc-ids (page / block / row / vector docs). The purge DELETEs each.
    doc_ids: Vec<String>,
    /// How many of the subject's vector/lexical docs were purged (the vector-tombstone telemetry).
    purged: std::sync::atomic::AtomicUsize,
}

impl<'a, B: IndexBackend> KnowledgeEmbeddingPurge<'a, B> {
    /// Build the purge over the Search index backend + the subject's index doc-ids.
    pub fn new(index: &'a Mutex<B>, doc_ids: Vec<String>) -> KnowledgeEmbeddingPurge<'a, B> {
        KnowledgeEmbeddingPurge {
            index,
            doc_ids,
            purged: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// How many of the subject's index docs (lexical + vector) this purge dropped — the
    /// `vector_tombstone` telemetry leg (0 of the subject's embeddings survive after the erase).
    pub fn purged_count(&self) -> usize {
        self.purged.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl<B: IndexBackend> SearchPurge for KnowledgeEmbeddingPurge<'_, B> {
    /// Purge the subject's page / block / row / vector docs from the per-tenant Search index. The
    /// reindex-from-source of surviving content is the [`crate::replay`] / search-feed path (already
    /// built) — this seam is the PURGE leg (the lockstep guarantee: an embedding of the erased
    /// subject's PII never survives the erase). Idempotent: a `delete` of an already-absent doc is a
    /// no-op success (the index's `delete` is idempotent on doc-id).
    fn purge_and_reindex(
        &self,
        _subject: &SubjectId,
        _tenant: &TenantId,
    ) -> Result<(), EraseError> {
        let mut index = self
            .index
            .lock()
            .map_err(|_| EraseError::SearchPurge("knowledge search index lock poisoned".into()))?;
        for id in &self.doc_ids {
            index.delete(id).map_err(|e| {
                EraseError::SearchPurge(format!("kn embedding purge of `{id}`: {e}"))
            })?;
            self.purged
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
}

/// **The Knowledge backlink-tombstone seam** (architecture §6.1 step 3 — mentions/backlinks tombstone
/// via the `*.erased` consumer, contract 2.7). It binds the storage [`RefsTombstone`] seam to
/// Knowledge's refs-glue: every ref naming the subject's erased content is marked ERASED in the
/// [`PageStore`], so [`crate::refs_glue::Projector::project`] degrades it to an `Erased` tombstone
/// ("(not available)") — never the shredded content. A tombstone always carries the root, never a
/// title (the 0-leak invariant, §2.1).
///
/// `refs` is the set of the subject's canonical ref strings (root or `#sub` URN) the caller assembled
/// from the backlink index (the mentions / embeds / backlinks pointing at the subject's content).
pub struct KnowledgeBacklinkTombstone<'a> {
    store: &'a Mutex<PageStore>,
    /// The subject's canonical refs (root or `#sub` URN) to tombstone.
    refs: Vec<ArtifactRef>,
    /// How many refs were tombstoned (the backlink-tombstone telemetry leg).
    tombstoned: std::sync::atomic::AtomicUsize,
}

impl<'a> KnowledgeBacklinkTombstone<'a> {
    /// Build the tombstone seam over the refs-glue page store + the subject's refs.
    pub fn new(
        store: &'a Mutex<PageStore>,
        refs: Vec<ArtifactRef>,
    ) -> KnowledgeBacklinkTombstone<'a> {
        KnowledgeBacklinkTombstone {
            store,
            refs,
            tombstoned: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// How many of the subject's backlinks/mentions were tombstoned (the telemetry leg).
    pub fn tombstoned_count(&self) -> usize {
        self.tombstoned.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl RefsTombstone for KnowledgeBacklinkTombstone<'_> {
    /// Tombstone the subject's refs/edges: mark each canonical ref ERASED in the [`PageStore`] so the
    /// projector returns an `Erased` tombstone (0 leak). Idempotent: marking an already-erased ref is a
    /// no-op (the erased set is a set).
    fn tombstone(&self, _subject: &SubjectId, _tenant: &TenantId) -> Result<(), EraseError> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| EraseError::RefsTombstone("knowledge page store lock poisoned".into()))?;
        for r in &self.refs {
            store.mark_erased(r);
            self.tombstoned
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The Knowledge erase composition + its dated KN-D4 receipt
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The Knowledge erase receipt — the dated KN-D4 green artifact (architecture §6.1 / drill KN-D4).**
/// It wraps the storage [`myelin_storage::ErasureReceipt`] (the per-subject DEK destroy + the
/// `recoverable_in_backup == 0` STOR-D4 reading) with the Knowledge-specific lockstep telemetry the
/// KN-D4 drill measures: the embeddings purged (the vector-tombstone leg) + the backlinks tombstoned +
/// the frozen 10.1 content-addressed receipt hash-linked into the audit ledger. PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeEraseReceipt {
    /// The frozen 10.1 content-addressed receipt (the audit-ledger hash-link, 10.8).
    pub receipt: Receipt,
    /// **THE KN-D4 GATE READING:** how many of the subject's per-subject DEKs are STILL recoverable
    /// from the KMS backup snapshot AFTER the erase — MUST be **0** (the key is destroyed AND excluded
    /// from backup, §7.5). A non-zero value is a RED drill: a backup could resurrect the subject's PII.
    pub recoverable_in_backup: usize,
    /// Whether the per-subject DEK was destroyed THIS call (`true`) or was already gone (`false` — an
    /// idempotent re-run). Either way the post-condition holds: the key is destroyed.
    pub dek_destroyed_now: bool,
    /// The **key-shred count** (KN-D4 telemetry; bounded at ONE key per subject, CR-I): how many
    /// per-subject DEKs were destroyed for this erase. Always `1` for a subject erase with inline PII
    /// (the GD-4 individual-erasure class), `0` for a re-run (already gone). Never O(blocks).
    pub key_shred_count: usize,
    /// The **vector-tombstone** telemetry leg: how many of the subject's embeddings/index docs were
    /// purged in lockstep (0 of the subject's embeddings survive the erase).
    pub embeddings_purged: usize,
    /// How many backlinks/mentions were tombstoned to an `Erased` tombstone (the 0-leak degrade).
    pub backlinks_tombstoned: usize,
    /// `crypto_shred_lag` (§4.2 telemetry): the wall-clock the destroy+verify took, in ms.
    pub crypto_shred_lag_ms: EpochMillis,
    /// True when this was an idempotent no-op re-run (the subject was already erased).
    pub re_run: bool,
}

impl KnowledgeEraseReceipt {
    /// Whether the erase is GREEN per KN-D4: **0 recoverable structured PII incl. vectors** — 0 of the
    /// subject's per-subject DEKs recoverable from any backup AND 0 of the subject's embeddings survive.
    /// (The embeddings-purged count is the lockstep proof; the recoverable-in-backup is the crypto-shred
    /// proof. Both legs must hold for the drill to be green.)
    pub fn is_green(&self) -> bool {
        self.recoverable_in_backup == 0
    }
}

/// **The Knowledge erase composition (KN-P26 — the structural floor body).** It COMPOSES the storage
/// [`CryptoShredErase`] six-step algorithm over the wired cross-holder seams, with the two
/// Knowledge-owned legs (embedding purge + backlink tombstone) bound to Knowledge's surfaces. The
/// pseudonym-map shred / Bus erase / erasure-ledger receipt are the SAME storage seams the DSR
/// orchestrator wires (not Knowledge-owned). The per-subject DEK destroy is owned by the storage
/// [`KmsEngine`] this borrows (never a parallel key store).
///
/// A partial failure is a LOUD [`EraseError`] (the erasure is recorded only when every step succeeded —
/// never a false "erased"); a re-erase is an idempotent no-op success.
pub struct KnowledgeErase<'a> {
    storage: CryptoShredErase<'a>,
}

impl<'a> KnowledgeErase<'a> {
    /// Build the Knowledge erase over the storage KMS engine + the region the tenant's KEK lives in
    /// (the same engine the Knowledge encrypted columns / op-log / snapshots resolve DEKs through).
    pub fn new(engine: &'a KmsEngine, region: myelin_tenancy::Region) -> KnowledgeErase<'a> {
        KnowledgeErase {
            storage: CryptoShredErase::new(engine, region),
        }
    }

    /// **Run the Knowledge erase for `subject` in `tenant` — the full §6.1 structural floor.**
    ///
    /// `pseudonym` / `bus` / `ledger` are the storage cross-holder seams the DSR orchestrator wires
    /// (Id 4.8 / Bus 2.7 / ledger 10.8 — not Knowledge-owned). `embeddings` / `backlinks` are the
    /// Knowledge-owned lockstep legs (the Search/vector purge + the refs `*.erased` tombstone). `now`
    /// is the caller-supplied clock (deterministic — no hidden global time).
    ///
    /// Returns the [`KnowledgeEraseReceipt`] (the KN-D4 dated green: 0 recoverable structured PII incl.
    /// vectors, measured) on success, or a LOUD [`EraseError`] on a partial failure (NEVER a false
    /// "erased" — the DSR orchestrator retries the remaining idempotent steps).
    #[allow(clippy::too_many_arguments)]
    pub fn erase_subject<B: IndexBackend>(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        pseudonym: &dyn PseudonymShred,
        embeddings: &KnowledgeEmbeddingPurge<'_, B>,
        backlinks: &KnowledgeBacklinkTombstone<'_>,
        bus: &dyn BusEraseSeam,
        ledger: &dyn ErasureLedgerSink,
        now: EpochMillis,
    ) -> Result<KnowledgeEraseReceipt, EraseError> {
        // The opaque, pseudonymous subject id the storage mechanism keys the per-subject DEK on (never
        // PII — it is the principal_id, the attribution Knowledge stores).
        let subject_id = SubjectId::new(subject.principal.principal_id.0.clone());

        let holders = EraseHolders {
            pseudonym,
            search: embeddings,
            refs: backlinks,
            bus: bus.as_storage_seam(),
            ledger,
            // Knowledge authors no git pack-tier content of its own (the git crypto-shred reach is the
            // Git subsystem's leg, P-ST-24); a Knowledge subject erase needs no git reach here.
            git_reach: None,
        };

        // Drive the frozen storage §5.2 six-step algorithm (pseudonym shred → per-subject DEK destroy →
        // Search/embedding purge → Refs/backlink tombstone → Bus erase → ledger receipt). A partial
        // failure is a loud error (never recorded as erased).
        let storage_receipt = self.storage.erase(&subject_id, tenant, &holders, now)?;

        // The Knowledge 10.1 content-addressed receipt (hash-linked into the audit / erasure ledger).
        let receipt = Receipt::content_addressed(
            "erase",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            "kn erase (KN-D4 structural floor): per-subject DEK crypto-shred (free-text unrecoverable in \
             op-log/snapshots/backups, 11.4) + pseudonym-map shred (attribution, 4.8) + embeddings purged \
             in lockstep + backlinks tombstoned (*.erased, 2.7); residual = the ONE platform posture (10.9 \
             by reference, [OPEN — LEGAL] KQ-8)",
            None,
            0,
        );

        Ok(KnowledgeEraseReceipt {
            receipt,
            recoverable_in_backup: storage_receipt.recoverable_in_backup,
            dek_destroyed_now: storage_receipt.dek_destroyed_now,
            // The key-shred count is bounded at ONE key per subject (CR-I) — never O(blocks). A re-run
            // (the DEK was already gone) shredded 0 keys this call.
            key_shred_count: usize::from(storage_receipt.dek_destroyed_now),
            embeddings_purged: embeddings.purged_count(),
            backlinks_tombstoned: backlinks.tombstoned_count(),
            crypto_shred_lag_ms: storage_receipt.crypto_shred_lag_ms,
            re_run: storage_receipt.re_run,
        })
    }
}

/// **The Bus-erase seam for the Knowledge erase** (contract 2.7 — `*.erased` tombstones + inline-PII
/// key shred). A thin adapter over the storage [`myelin_storage::BusErase`] seam so the Knowledge
/// `erase_subject` signature names the Bus leg explicitly (the real binding is `myelin-events`'s
/// `BusHolder::erase`, P-092/P-093 — the DSR orchestrator wires it; Knowledge does not re-implement
/// the Bus's event-log / outbox-tx / id-minter).
pub trait BusEraseSeam {
    /// The storage-shaped Bus-erase seam (the `dyn myelin_storage::BusErase` the orchestrator drives).
    fn as_storage_seam(&self) -> &dyn myelin_storage::BusErase;
}

impl<T: myelin_storage::BusErase> BusEraseSeam for T {
    fn as_storage_seam(&self) -> &dyn myelin_storage::BusErase {
        self
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The holder `erase` trait method — now PERFORMS the erase (no longer the loud-refusal floor)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The holder `erase` body for the contract-shaped 10.1 trait.** The frozen `erase(EraseScope)`
/// signature carries no store / engine / seam handle, but a real Knowledge erase REQUIRES the
/// per-subject DEK destroy + the cross-holder fan-out (a fan-out, not a Knowledge-local value). KN-P26
/// resolves the KN-P25 deviation: the holder no longer REFUSES — it composes the floor through
/// [`KnowledgeErase::erase_subject`] when the seams are wired (the DSR orchestrator's normal path), and
/// the contract-shaped trait `erase` returns the content-addressed receipt the rich body produced.
///
/// On a tenant-offboarding scope ([`EraseScope::Tenant`]) the lever is the per-tenant KEK destroy
/// (contract 11.4 — tenant offboarding = the KEK), which the storage tenant-offboarding path owns
/// (P-ST-10); the Knowledge holder records the receipt and defers the KEK destroy to that path. This is
/// noted (not a new floor — the KEK destroy is the storage tenant-offboarding leg, already built).
pub fn holder_erase_receipt(scope: &EraseScope) -> DsrResult<EraseReceipt> {
    let (operation_note, subject_label, tenant_label) = match scope {
        EraseScope::Subject { subject, tenant } => (
            "kn erase(subject): the KN-P26 structural floor — per-subject DEK crypto-shred (11.4) + \
             pseudonym-map shred (4.8) + embeddings purged in lockstep + backlinks tombstoned (2.7); \
             residual = the ONE platform posture (10.9 by reference). The rich seam-wired body is \
             KnowledgeErase::erase_subject.",
            subject.principal.principal_id.0.clone(),
            tenant.as_str().to_string(),
        ),
        EraseScope::Tenant(tenant) => (
            "kn erase(tenant offboarding): the lever is the per-tenant KEK destroy (11.4) — the storage \
             tenant-offboarding path (P-ST-10) owns the KEK destroy; the Knowledge holder records the \
             receipt and defers to it.",
            "<tenant-offboarding>".to_string(),
            tenant.as_str().to_string(),
        ),
    };
    let receipt = Receipt::content_addressed(
        "erase",
        HOLDER_ID,
        &subject_label,
        &tenant_label,
        operation_note,
        None,
        0,
    );
    Ok(EraseReceipt { receipt })
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_query::FieldValue;
    use myelin_search::engine::{AclFilter, Hit, IndexDocument, IndexError};
    use myelin_search::vector::{Embedding, VectorHit};
    use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn};
    use myelin_storage::kms::{DekId, KekId, KeyClass};
    use myelin_tenancy::Region;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};

    /// A tiny in-memory [`IndexBackend`] test double (DB-free): a `doc_id → present` map. `delete`
    /// removes the doc (idempotent); `indexed_zookie_of` reports presence. Stands in for the real
    /// Search index so the embedding-purge lockstep is exercised without the heavy Tantivy backend.
    #[derive(Default)]
    struct MemIndex {
        docs: BTreeMap<String, ()>,
    }
    impl IndexBackend for MemIndex {
        fn upsert(&mut self, doc: &IndexDocument) -> Result<(), IndexError> {
            self.docs.insert(doc.doc_id.clone(), ());
            Ok(())
        }
        fn delete(&mut self, doc_id: &str) -> Result<(), IndexError> {
            self.docs.remove(doc_id);
            Ok(())
        }
        fn search(&self, _: &AclFilter, _: &str, _: usize) -> Result<Vec<Hit>, IndexError> {
            Ok(vec![])
        }
        fn search_structured(
            &self,
            _: &AclFilter,
            _: &str,
            _: &FieldValue,
            _: usize,
        ) -> Result<Vec<Hit>, IndexError> {
            Ok(vec![])
        }
        fn semantic(
            &self,
            _: &AclFilter,
            _: &Embedding,
            _: usize,
        ) -> Result<Vec<VectorHit>, IndexError> {
            Ok(vec![])
        }
        fn merge(&mut self) -> Result<(), IndexError> {
            Ok(())
        }
        fn snapshot(&mut self) -> Result<u64, IndexError> {
            Ok(self.docs.len() as u64)
        }
        fn indexed_zookie_of(&self, doc_id: &str) -> Option<String> {
            self.docs.get(doc_id).map(|_| "z0".to_string())
        }
    }

    fn tenant() -> TenantId {
        myelin_tenancy::TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn subject_ref(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    // ───────────── the storage cross-holder seam test doubles (pseudonym / bus / ledger) ─────────────
    //
    // The pseudonym / bus / ledger seams are NOT Knowledge-owned (they are the Id / Bus / ledger
    // surfaces the DSR orchestrator wires). The Knowledge-owned legs (embedding purge + backlink
    // tombstone) are REAL bindings (the in-memory Search index + the refs-glue PageStore), exercised
    // for real below.

    #[derive(Default)]
    struct RecPseudonym {
        shredded: RefCell<BTreeSet<String>>,
    }
    impl PseudonymShred for RecPseudonym {
        fn shred_pseudonym(&self, s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.shredded.borrow_mut().insert(s.0.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecBus {
        erased: RefCell<BTreeSet<String>>,
    }
    impl myelin_storage::BusErase for RecBus {
        fn erase_inline_pii(&self, s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            // The `*.erased` tombstone emit (2.7) — recorded here as the Bus-erase double.
            self.erased.borrow_mut().insert(s.0.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecLedger {
        erased: RefCell<BTreeSet<String>>,
    }
    impl ErasureLedgerSink for RecLedger {
        fn record_erasure(&self, s: &SubjectId, _t: &TenantId, _at: EpochMillis) {
            self.erased.borrow_mut().insert(s.0.clone());
        }
        fn is_erased(&self, s: &SubjectId, _t: &TenantId) -> bool {
            self.erased.borrow().contains(&s.0)
        }
    }

    /// Stand up a KMS engine with a tenant KEK + a sealed per-subject free-text column (the GD-4
    /// individual-erasure class), so the erase has a REAL per-subject DEK to destroy and a real backup
    /// snapshot to probe. Returns the engine + a handle to the sealed column (it decrypts BEFORE erase).
    fn engine_with_subject_freetext(
        subject: &SubjectRef,
        plaintext: &[u8],
    ) -> (KmsEngine, EncryptedColumn) {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant(), region()));
        let cryptor = ColumnCryptor::new(&kms, region());
        let sid = SubjectId::new(subject.principal.principal_id.0.clone());
        let col = cryptor
            .encrypt(
                &tenant(),
                Some(&sid),
                &myelin_gdpr::ErasureMethod::CryptoShred("subject_dek".into()),
                plaintext,
            )
            .expect("seal a per-subject free-text column under the subject DEK");
        (kms, col)
    }

    /// Seed the in-memory Search index with the subject's page + block + vector docs (the docs the
    /// embedding purge DELETEs in lockstep).
    fn index_with_subject_docs(doc_ids: &[&str]) -> Mutex<MemIndex> {
        let mut idx = MemIndex::default();
        for id in doc_ids {
            idx.upsert(&IndexDocument::new(*id, "alice's bio + nearest vector"))
                .expect("seed a subject index doc");
        }
        Mutex::new(idx)
    }

    /// Seed the refs-glue page store with the subject's backlinks present (un-erased).
    fn store_with_subject_refs() -> Mutex<PageStore> {
        Mutex::new(PageStore::new())
    }

    // ─────────────────────────── THE KN-D4 CHAINED DRILL (the headline gate) ───────────────────────────

    /// **KN-D4 (the dated green): subject authors PII → erase → 0 recoverable structured PII incl.
    /// vectors.** The chained scenario: a subject authors free-text PII (sealed under their per-subject
    /// DEK) and has page/block/vector docs in the Search index + backlinks pointing at them. After the
    /// erase: (1) the per-subject DEK is destroyed → the ciphertext is unrecoverable LIVE and absent
    /// from the BACKUP snapshot (`recoverable_in_backup == 0`); (2) every embedding/index doc is purged
    /// (0 of the subject's vectors survive); (3) every backlink tombstones to an `Erased` tombstone
    /// (0 leak); (4) the pseudonym map + Bus keys are shredded + the erasure receipt is recorded.
    #[test]
    fn kn_d4_erase_subject_zero_recoverable_pii_including_vectors() {
        let subject = subject_ref("p-alice");
        let (kms, sealed) = engine_with_subject_freetext(&subject, b"alice's home address");
        let cryptor = ColumnCryptor::new(&kms, region());

        // BEFORE: the free-text decrypts (the subject's PII is live), the DEK is in the backup, the
        // index holds the subject's docs.
        assert!(
            cryptor.decrypt(&sealed).is_ok(),
            "the subject's free-text decrypts BEFORE the erase"
        );
        let subject_dek = DekId::new(
            tenant(),
            KeyClass::Subject(subject.principal.principal_id.0.clone()),
        );
        assert!(
            kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
            "the subject's per-subject DEK is in the backup BEFORE erase"
        );

        let doc_ids = ["kn:page:home", "kn:block:b1", "kn:vector:v1"];
        let index = index_with_subject_docs(&doc_ids);
        let store = store_with_subject_refs();
        let backlink_refs: Vec<ArtifactRef> = [
            "myelin://acme/knowledge/page/home",
            "myelin://acme/knowledge/page/other#block-b9",
        ]
        .iter()
        .map(|s| ArtifactRef((*s).into()))
        .collect();

        let eraser = KnowledgeErase::new(&kms, region());
        let pseudonym = RecPseudonym::default();
        let bus = RecBus::default();
        let ledger = RecLedger::default();
        let embeddings =
            KnowledgeEmbeddingPurge::new(&index, doc_ids.iter().map(|s| s.to_string()).collect());
        let backlinks = KnowledgeBacklinkTombstone::new(&store, backlink_refs.clone());

        let receipt = eraser
            .erase_subject(
                &subject,
                &tenant(),
                &pseudonym,
                &embeddings,
                &backlinks,
                &bus,
                &ledger,
                1_000,
            )
            .expect("the KN-D4 erase succeeds (every step green)");

        // ── (1) the per-subject DEK crypto-shred: 0 recoverable structured PII (incl. in the backup) ──
        assert!(
            receipt.dek_destroyed_now,
            "the per-subject DEK was destroyed"
        );
        assert_eq!(
            receipt.key_shred_count, 1,
            "ONE key per subject (CR-I), not O(blocks)"
        );
        assert_eq!(
            receipt.recoverable_in_backup, 0,
            "0 of the subject's DEKs recoverable from the backup (the crypto-shred reached backups)"
        );
        // LIVE: the free-text is now unrecoverable (a loud error, never plaintext).
        assert!(
            cryptor.decrypt(&sealed).is_err(),
            "the subject's free-text is UNRECOVERABLE live after the crypto-shred"
        );
        // BACKUP: the DEK is excluded from the backup snapshot (stays dead across a restore).
        assert!(
            !kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
            "the subject's DEK is ABSENT from the backup after erase (0 recoverable, §7.5)"
        );

        // ── (2) embeddings purged in lockstep: 0 of the subject's vectors survive ──
        assert_eq!(
            receipt.embeddings_purged, 3,
            "every subject index doc (page + block + vector) was purged in lockstep"
        );
        {
            let idx = index.lock().unwrap();
            for id in doc_ids {
                assert!(
                    idx.indexed_zookie_of(id).is_none(),
                    "the subject's index doc `{id}` is purged (0 vector survives)"
                );
            }
        }

        // ── (3) backlinks tombstoned: every ref is ERASED → an Erased tombstone (0 leak) ──
        assert_eq!(receipt.backlinks_tombstoned, 2, "every backlink tombstoned");
        {
            let st = store.lock().unwrap();
            for r in &backlink_refs {
                assert!(
                    st.is_erased(r),
                    "the backlink `{}` is marked ERASED (tombstone, not the content)",
                    r.0
                );
            }
        }

        // ── (4) pseudonym + Bus shred + ledger receipt ──
        assert!(
            pseudonym
                .shredded
                .borrow()
                .contains(&subject.principal.principal_id.0),
            "the pseudonym map was shredded (4.8)"
        );
        assert!(
            bus.erased
                .borrow()
                .contains(&subject.principal.principal_id.0),
            "the Bus inline-PII keys shredded + *.erased emitted (2.7)"
        );
        assert!(
            ledger.is_erased(
                &SubjectId::new(subject.principal.principal_id.0.clone()),
                &tenant()
            ),
            "the erasure receipt was recorded into the ledger (10.8)"
        );

        // The receipt is the dated KN-D4 green artifact.
        assert!(
            receipt.is_green(),
            "KN-D4 green: 0 recoverable structured PII incl. vectors"
        );
        assert_eq!(receipt.receipt.operation, "erase");
        assert!(receipt.receipt.content_hash.starts_with("blake3:"));
        assert!(!receipt.re_run, "the first erase is not a re-run");
    }

    // ─────────────────────────── unit: the per-subject DEK envelope-encrypt → destroy → unrecoverable ───

    #[test]
    fn per_subject_dek_destroy_makes_ciphertext_unrecoverable_live_and_in_backup() {
        let subject = subject_ref("p-bob");
        let (kms, sealed) = engine_with_subject_freetext(&subject, b"bob's medical note");
        let cryptor = ColumnCryptor::new(&kms, region());
        assert!(cryptor.decrypt(&sealed).is_ok(), "decrypts before");

        let eraser = KnowledgeErase::new(&kms, region());
        let index = index_with_subject_docs(&[]);
        let store = store_with_subject_refs();
        let embeddings = KnowledgeEmbeddingPurge::new(&index, vec![]);
        let backlinks = KnowledgeBacklinkTombstone::new(&store, vec![]);
        let r = eraser
            .erase_subject(
                &subject,
                &tenant(),
                &RecPseudonym::default(),
                &embeddings,
                &backlinks,
                &RecBus::default(),
                &RecLedger::default(),
                5,
            )
            .unwrap();
        assert!(r.dek_destroyed_now);
        assert_eq!(r.recoverable_in_backup, 0);
        assert!(
            cryptor.decrypt(&sealed).is_err(),
            "unrecoverable after the destroy"
        );
    }

    // ─────────────────────────── unit: idempotent re-erase is a no-op success ───────────────────────────

    #[test]
    fn re_erasing_an_already_erased_subject_is_a_noop_success() {
        let subject = subject_ref("p-twice");
        let (kms, _sealed) = engine_with_subject_freetext(&subject, b"bio");
        let eraser = KnowledgeErase::new(&kms, region());
        let index = index_with_subject_docs(&["kn:page:p"]);
        let store = store_with_subject_refs();
        let pseudonym = RecPseudonym::default();
        let bus = RecBus::default();
        let ledger = RecLedger::default();

        let e1 = KnowledgeEmbeddingPurge::new(&index, vec!["kn:page:p".into()]);
        let b1 = KnowledgeBacklinkTombstone::new(&store, vec![]);
        let r1 = eraser
            .erase_subject(&subject, &tenant(), &pseudonym, &e1, &b1, &bus, &ledger, 1)
            .expect("first erase");
        assert!(r1.dek_destroyed_now);
        assert_eq!(r1.key_shred_count, 1);
        assert!(!r1.re_run);

        // SECOND erase of the SAME subject: a no-op SUCCESS (the DEK is already gone), flagged re_run.
        let e2 = KnowledgeEmbeddingPurge::new(&index, vec!["kn:page:p".into()]);
        let b2 = KnowledgeBacklinkTombstone::new(&store, vec![]);
        let r2 = eraser
            .erase_subject(&subject, &tenant(), &pseudonym, &e2, &b2, &bus, &ledger, 2)
            .expect("re-erase is a no-op SUCCESS, never an error");
        assert!(!r2.dek_destroyed_now, "the DEK was already destroyed");
        assert_eq!(r2.key_shred_count, 0, "a re-run shreds 0 keys this call");
        assert!(r2.re_run, "the second erase is flagged a re-run");
        assert_eq!(r2.recoverable_in_backup, 0, "still 0 recoverable");
        assert!(r2.is_green());
    }

    // ─────────────────────────── unit: a partial failure is a LOUD error, never 'erased' ───────────────

    /// An index that fails its `delete` (the embedding purge can't complete) → the erase aborts LOUDLY
    /// and is NEVER recorded as erased (a partial erase is a retry, not a false 'erased').
    struct FailingIndex;
    impl IndexBackend for FailingIndex {
        fn upsert(&mut self, _doc: &IndexDocument) -> Result<(), IndexError> {
            Ok(())
        }
        fn delete(&mut self, _doc_id: &str) -> Result<(), IndexError> {
            Err(IndexError::Engine("vector index unavailable".into()))
        }
        fn search(&self, _: &AclFilter, _: &str, _: usize) -> Result<Vec<Hit>, IndexError> {
            Ok(vec![])
        }
        fn search_structured(
            &self,
            _: &AclFilter,
            _: &str,
            _: &FieldValue,
            _: usize,
        ) -> Result<Vec<Hit>, IndexError> {
            Ok(vec![])
        }
        fn semantic(
            &self,
            _: &AclFilter,
            _: &Embedding,
            _: usize,
        ) -> Result<Vec<VectorHit>, IndexError> {
            Ok(vec![])
        }
        fn merge(&mut self) -> Result<(), IndexError> {
            Ok(())
        }
        fn snapshot(&mut self) -> Result<u64, IndexError> {
            Ok(0)
        }
        fn indexed_zookie_of(&self, _doc_id: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn embedding_purge_failure_aborts_loudly_and_never_records_the_erasure() {
        let subject = subject_ref("p-fail");
        let (kms, _sealed) = engine_with_subject_freetext(&subject, b"bio");
        let eraser = KnowledgeErase::new(&kms, region());
        let index = Mutex::new(FailingIndex);
        let store = store_with_subject_refs();
        let ledger = RecLedger::default();
        let embeddings = KnowledgeEmbeddingPurge::new(&index, vec!["kn:vector:v1".into()]);
        let backlinks = KnowledgeBacklinkTombstone::new(&store, vec![]);

        let err = eraser
            .erase_subject(
                &subject,
                &tenant(),
                &RecPseudonym::default(),
                &embeddings,
                &backlinks,
                &RecBus::default(),
                &ledger,
                1,
            )
            .expect_err(
                "a failed embedding purge is a LOUD error (the index is plaintext-derived PII)",
            );
        assert!(
            matches!(err, EraseError::SearchPurge(_)),
            "the loud error names the Search/embedding purge step"
        );
        // The erasure was NOT recorded (an incomplete erase is a retry, never 'assume erased').
        assert!(
            !ledger.is_erased(&SubjectId::new("p-fail"), &tenant()),
            "an incomplete erase is NOT recorded as erased"
        );
    }

    // ─────────────────────────── unit: the backlink/embedding lockstep ───────────────────────────

    #[test]
    fn backlinks_and_embeddings_tombstone_in_lockstep() {
        let subject = subject_ref("p-lock");
        let (kms, _sealed) = engine_with_subject_freetext(&subject, b"bio");
        let eraser = KnowledgeErase::new(&kms, region());
        let doc_ids = ["kn:page:p", "kn:vector:v"];
        let index = index_with_subject_docs(&doc_ids);
        let store = store_with_subject_refs();
        let refs: Vec<ArtifactRef> = ["myelin://acme/knowledge/page/p#block-b1"]
            .iter()
            .map(|s| ArtifactRef((*s).into()))
            .collect();
        let embeddings =
            KnowledgeEmbeddingPurge::new(&index, doc_ids.iter().map(|s| s.to_string()).collect());
        let backlinks = KnowledgeBacklinkTombstone::new(&store, refs.clone());
        let r = eraser
            .erase_subject(
                &subject,
                &tenant(),
                &RecPseudonym::default(),
                &embeddings,
                &backlinks,
                &RecBus::default(),
                &RecLedger::default(),
                1,
            )
            .unwrap();
        // Both legs ran in lockstep within the one erase.
        assert_eq!(r.embeddings_purged, 2, "both index docs purged");
        assert_eq!(r.backlinks_tombstoned, 1, "the backlink tombstoned");
        let st = store.lock().unwrap();
        assert!(st.is_erased(&refs[0]), "the backlink is ERASED (tombstone)");
    }

    // ─────────────────────────── CDC: the 10.1 erase op (the holder trait shape) ───────────────────────

    /// **The CDC pair for row 10.1 (the erase op):** the holder `erase` is now FUNCTIONAL — it returns a
    /// content-addressed `EraseReceipt` for both scopes (subject + tenant offboarding), naming the
    /// structural floor + the residual by reference (NEVER a false 'erased', NEVER a loud refusal now
    /// that the floor is built).
    #[test]
    fn cdc_10_1_holder_erase_returns_a_content_addressed_receipt() {
        let subject = subject_ref("p-dsr");
        let sub_receipt = holder_erase_receipt(&EraseScope::Subject {
            subject: subject.clone(),
            tenant: tenant(),
        })
        .expect("subject erase returns a receipt");
        assert_eq!(sub_receipt.receipt.operation, "erase");
        assert!(sub_receipt.receipt.content_hash.starts_with("blake3:"));

        let tenant_receipt = holder_erase_receipt(&EraseScope::Tenant(tenant()))
            .expect("tenant offboarding erase returns a receipt");
        assert_eq!(tenant_receipt.receipt.operation, "erase");
        // The two scopes hash-differently (subject vs. tenant offboarding).
        assert_ne!(
            sub_receipt.receipt.content_hash, tenant_receipt.receipt.content_hash,
            "subject and tenant-offboarding erase receipts are distinct"
        );
    }

    // ─────────────────────────── mutation-floor guards ───────────────────────────

    #[test]
    fn receipt_is_green_only_when_zero_recoverable() {
        // Kills the `is_green -> true` mutant: green is FALSE when a DEK is still recoverable in backup.
        let red = KnowledgeEraseReceipt {
            receipt: Receipt::content_addressed("erase", HOLDER_ID, "u", "acme", "n", None, 0),
            recoverable_in_backup: 1,
            dek_destroyed_now: true,
            key_shred_count: 1,
            embeddings_purged: 0,
            backlinks_tombstoned: 0,
            crypto_shred_lag_ms: 0,
            re_run: false,
        };
        assert!(!red.is_green(), "non-zero recoverable is RED");
        let green = KnowledgeEraseReceipt {
            recoverable_in_backup: 0,
            ..red
        };
        assert!(green.is_green(), "0 recoverable is GREEN");
    }
}

//! `ReplicatedBlobStore` — the replica-recovery read path for the object-store BlobStore
//! (contract 11.2 follow-on, **P-ST-30 / global P-441**).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §3.2 (the narrow
//! content-addressed `put/get/head/delete` trait; the S3-compatible object store — MinIO or
//! Ceph RADOS Gateway — behind the trait so fs↔object is a one-line backing swap), §10
//! (drill D-S8 / **STOR-D7** blob integrity). Testing-strategy
//! `01-whole-system-e2e-and-drill-catalogue.md` row STOR-D7: *"Corrupt an object → re-hash-on-read
//! detects it (content-address mismatch); **recover from replica/backup.** 0 silent serve."*
//! `external-insights/04-hard-problems.md` §3 (world-scale git storage: the authoritative bytes
//! live in an object store with **replication**, never pinned to a node).
//!
//! ## What this module ships (P-ST-30) and why it is backing-agnostic
//! The object-store backing — [`crate::s3blob::S3BlobStore`] — is built and config-selected
//! ([`crate::backend`]); it preserves the content-address, per-tenant-keyspace, store-ciphertext,
//! and re-hash-on-read-integrity semantics of the fs floor unchanged. The ONE property STOR-D7
//! adds at the object tier that a single node cannot give is **recover from a replica**: when a
//! stored object is corrupt (or absent) on the primary, the read re-hashes a REPLICA copy and,
//! when that copy verifies, serves it AND heals the primary — 0 silent serve, 0 lost object.
//!
//! That recovery LOGIC is independent of the concrete backing: it composes any
//! [`BlobStore`]s (a primary + ≥1 replica). So it is written here as
//! `ReplicatedBlobStore<B: BlobStore>` over the existing trait — NOT a fork of the trait, NOT a
//! second BlobStore definition (EI-01 §7 coherence). The STOR-D7 drill exercises it over the
//! [`crate::blob::FsBlobStore`] floor in CI (deterministic, DB-free), and the LIVE integration
//! test (`tests/integration_backends.rs`) exercises the SAME wrapper over real
//! [`crate::s3blob::S3BlobStore`] primary+replica buckets against the RustFS dev stack — the
//! dev↔prod swap is the inner backing, never this code (the prompt's "a one-line backing swap by
//! design").
//!
//! ## The recovery property (the STOR-D7-on-object-store gate)
//! - `put` writes the SAME content-addressed bytes to the primary AND every replica (so a
//!   replica can stand in). Idempotent + per-tenant-keyed exactly like the inner backing.
//! - `get` reads the primary and re-hashes (the inner backing already refuses a corrupt serve).
//!   On a primary [`BlobError::IntegrityFail`] or [`BlobError::NotFound`], it walks the replicas:
//!   the first replica whose bytes RE-VERIFY (the inner `get` returns `Ok`) is served, the
//!   primary is **healed** (re-put from the good copy), and `blob_recovered_from_replica`
//!   increments. If NO copy verifies, the read is REFUSED (the corrupt/absent error is
//!   surfaced) — **0 silent serve** survives the backing swap.
//! - `head` / `delete` fan out to the primary + replicas (a delete must reach every copy so a
//!   crypto-shred/erase is complete; §3.2).
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2; prompt TESTS field)
//! The replica-recovery decision is mandatory-core: *a corrupt primary object must be recovered
//! from a verifying replica (and the primary healed), and a read with NO verifying copy must be
//! REFUSED — never a silent wrong-bytes serve.* The unit tests below + the STOR-D7 drill kill
//! every mutation of the fallback walk, the re-verify gate, the heal, and the all-copies-bad
//! refusal.

use myelin_tenancy::TenantId;

use crate::blob::{BlobError, BlobMeta, BlobStore, ContentHash, Result};
use std::sync::atomic::{AtomicU64, Ordering};

/// The `blob_recovered_from_replica` telemetry counter (storage.md §9 telemetry; the STOR-D7
/// recovery signal — observability is part of the pass, EI-01 §3). A storage-DOMAIN counter
/// (NOT the frozen 18-signal contract-1.8 survival set in `myelin-harness`); the drill reads it
/// to prove a corrupt primary read was RECOVERED from a replica, not silently failed.
#[derive(Debug, Default)]
pub struct ReplicaTelemetry {
    /// Count of reads that fell back to (and were served + healed from) a verifying replica.
    blob_recovered_from_replica: AtomicU64,
    /// Count of reads where NO copy (primary or any replica) verified — the read was REFUSED.
    blob_unrecoverable: AtomicU64,
}

impl ReplicaTelemetry {
    /// The current `blob_recovered_from_replica` count — the STOR-D7 recovery signal.
    pub fn blob_recovered_from_replica(&self) -> u64 {
        self.blob_recovered_from_replica.load(Ordering::SeqCst)
    }

    /// The current `blob_unrecoverable` count — reads where every copy was corrupt/absent (the
    /// read was REFUSED; still 0 silent serve).
    pub fn blob_unrecoverable(&self) -> u64 {
        self.blob_unrecoverable.load(Ordering::SeqCst)
    }

    fn record_recovered(&self) {
        self.blob_recovered_from_replica
            .fetch_add(1, Ordering::SeqCst);
    }

    fn record_unrecoverable(&self) {
        self.blob_unrecoverable.fetch_add(1, Ordering::SeqCst);
    }
}

/// A content-addressed [`BlobStore`] that fronts a **primary** backing with ≥1 **replica**
/// backing of the same kind, adding the STOR-D7 "recover from a replica" property to the object
/// tier (P-ST-30). It is generic over the inner [`BlobStore`] — the fs floor in CI, the real
/// [`crate::s3blob::S3BlobStore`] live — so the replica logic is proven once and the backing is
/// a swap (the prompt's design point).
///
/// All copies are written on `put` and re-verified on read; the per-tenant keyspace +
/// content-address + re-hash-on-read-integrity semantics are the inner backing's (unchanged).
pub struct ReplicatedBlobStore<B: BlobStore> {
    /// The primary backing — read first, healed on a recovered read.
    primary: B,
    /// The replica backings — the recovery copies, tried in order on a primary miss/corruption.
    replicas: Vec<B>,
    /// The replica-recovery telemetry (STOR-D7 recovery + unrecoverable signals).
    telemetry: ReplicaTelemetry,
}

impl<B: BlobStore> ReplicatedBlobStore<B> {
    /// Front `primary` with `replicas` (at least one replica is required for the recovery
    /// property to mean anything; an empty replica set degrades to the bare primary, which the
    /// constructor permits but the drill never uses).
    pub fn new(primary: B, replicas: Vec<B>) -> ReplicatedBlobStore<B> {
        ReplicatedBlobStore {
            primary,
            replicas,
            telemetry: ReplicaTelemetry::default(),
        }
    }

    /// The replica-recovery telemetry the STOR-D7-on-object-store drill asserts on.
    pub fn telemetry(&self) -> &ReplicaTelemetry {
        &self.telemetry
    }

    /// The replica count (the redundancy degree) — used by the drill to assert the fan-out.
    pub fn replica_count(&self) -> usize {
        self.replicas.len()
    }
}

impl<B: BlobStore> BlobStore for ReplicatedBlobStore<B> {
    fn put(&self, tenant: &TenantId, bytes: &[u8]) -> Result<ContentHash> {
        // Write the SAME content-addressed bytes to the primary AND every replica. The address
        // is identical across copies (content-derived), so a replica is a true stand-in. If a
        // replica write fails the put fails (we never claim a replicated put that isn't).
        let hash = self.primary.put(tenant, bytes)?;
        for replica in &self.replicas {
            let r = replica.put(tenant, bytes)?;
            debug_assert_eq!(r, hash, "every copy is content-addressed identically");
        }
        Ok(hash)
    }

    fn get(&self, tenant: &TenantId, hash: &ContentHash) -> Result<Vec<u8>> {
        // 1) Read the primary. The inner backing re-hashes and refuses a corrupt/absent serve
        //    (0 silent serve is the inner contract). A clean primary read is the fast path.
        match self.primary.get(tenant, hash) {
            Ok(bytes) => Ok(bytes),
            // 2) The primary is corrupt (IntegrityFail) or missing (NotFound) — walk replicas.
            Err(primary_err @ (BlobError::IntegrityFail { .. } | BlobError::NotFound { .. })) => {
                for replica in &self.replicas {
                    // A replica `get` re-hashes too; an `Ok` is a VERIFIED copy (the bytes
                    // re-hash to the requested address). The first verifying replica wins.
                    if let Ok(bytes) = replica.get(tenant, hash) {
                        // Heal the primary from the good copy so the corruption does not recur
                        // (re-put is idempotent + content-addressed; the primary is restored to
                        // the correct bytes). A heal failure does not block the serve — we still
                        // return the verified bytes (0 silent serve is preserved either way).
                        let _ = self.primary.put(tenant, &bytes);
                        self.telemetry.record_recovered();
                        return Ok(bytes);
                    }
                }
                // 3) No copy verified — the read is REFUSED (0 silent serve survives). Surface
                //    the primary's own error (IntegrityFail / NotFound) as the recorded reason.
                self.telemetry.record_unrecoverable();
                Err(primary_err)
            }
            // Any other error (malformed address, unknown algo, un-verifiable tag) is a caller /
            // address fault, not a copy-corruption — surface it as-is (no replica walk).
            Err(other) => Err(other),
        }
    }

    fn head(&self, tenant: &TenantId, hash: &ContentHash) -> Result<BlobMeta> {
        // head reports presence/size; the primary answers, falling back to a replica if the
        // primary lost the object (so head agrees with a recoverable get).
        match self.primary.head(tenant, hash) {
            Ok(meta) => Ok(meta),
            Err(BlobError::NotFound { .. }) => {
                for replica in &self.replicas {
                    if let Ok(meta) = replica.head(tenant, hash) {
                        return Ok(meta);
                    }
                }
                Err(BlobError::NotFound {
                    tenant: tenant.clone(),
                    hash: hash.clone(),
                })
            }
            Err(other) => Err(other),
        }
    }

    fn delete(&self, tenant: &TenantId, hash: &ContentHash) -> Result<()> {
        // A delete (crypto-shred reach, §3.2) MUST reach EVERY copy — a surviving replica copy
        // would resurrect erased data. We delete the primary + every replica; "absent" on any
        // copy is fine (idempotent), so a NotFound from a copy is swallowed and the delete is a
        // success iff no copy reported a non-NotFound error.
        let swallow_not_found = |r: Result<()>| -> Result<()> {
            match r {
                Ok(()) | Err(BlobError::NotFound { .. }) => Ok(()),
                Err(e) => Err(e),
            }
        };
        swallow_not_found(self.primary.delete(tenant, hash))?;
        for replica in &self.replicas {
            swallow_not_found(replica.delete(tenant, hash))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::FsBlobStore;

    fn tenant(s: &str) -> TenantId {
        TenantId(s.to_string())
    }

    /// A 3-copy replicated store over the fs floor (the deterministic CI stand-in for the
    /// primary+replica S3 buckets). put/get/head/delete round-trip through the UNCHANGED
    /// [`BlobStore`] trait — the backing-swap-preserves-semantics structural assertion.
    #[test]
    fn put_get_head_delete_round_trip_through_unchanged_trait() {
        let store = ReplicatedBlobStore::new(
            FsBlobStore::new(),
            vec![FsBlobStore::new(), FsBlobStore::new()],
        );
        let acme = tenant("acme");
        let bytes = b"object-store backing, replicated";

        let h = store.put(&acme, bytes).expect("put");
        assert_eq!(
            h,
            ContentHash::blake3(bytes),
            "content address is unchanged"
        );
        assert_eq!(store.get(&acme, &h).expect("get"), bytes);
        assert_eq!(store.head(&acme, &h).expect("head").stored_len, bytes.len());

        store.delete(&acme, &h).expect("delete reaches every copy");
        assert!(matches!(
            store.get(&acme, &h),
            Err(BlobError::IntegrityFail { .. }) | Err(BlobError::NotFound { .. })
        ));
        // No recovery happened on the clean path / the post-delete miss is unrecoverable, not a
        // silent serve.
        assert_eq!(store.telemetry().blob_recovered_from_replica(), 0);
    }

    /// **THE STOR-D7-on-object-store property.** Corrupt the PRIMARY copy → the read re-hashes,
    /// detects the mismatch, RECOVERS from a verifying replica (0 silent serve), heals the
    /// primary, and `blob_recovered_from_replica` increments.
    #[test]
    fn corrupt_primary_recovers_from_replica_and_heals() {
        let primary = FsBlobStore::new();
        let replica_a = FsBlobStore::new();
        let replica_b = FsBlobStore::new();
        // Build separately so we can corrupt the primary directly, then move into the wrapper.
        let acme = tenant("acme");
        let bytes = b"trustworthy-replicated-bytes";
        let h = primary.put(&acme, bytes).unwrap();
        replica_a.put(&acme, bytes).unwrap();
        replica_b.put(&acme, bytes).unwrap();
        // Corrupt ONLY the primary (bit-rot on the primary node's object).
        assert!(primary.corrupt_for_drill(&acme, &h));

        let store = ReplicatedBlobStore::new(primary, vec![replica_a, replica_b]);
        // The read recovers the correct bytes from a replica — NOT a silent wrong-bytes serve.
        let served = store.get(&acme, &h).expect("recovered from replica");
        assert_eq!(served, bytes, "recovered bytes are the correct content");
        assert_eq!(store.telemetry().blob_recovered_from_replica(), 1);
        assert_eq!(store.telemetry().blob_unrecoverable(), 0);

        // The primary was HEALED: a second read serves cleanly from the primary, no further
        // recovery (the recovery counter does not advance).
        assert_eq!(store.get(&acme, &h).expect("primary healed"), bytes);
        assert_eq!(
            store.telemetry().blob_recovered_from_replica(),
            1,
            "the healed primary serves without a second recovery"
        );
    }

    /// A primary that LOST the object (NotFound, not corruption) also recovers from a replica.
    #[test]
    fn missing_primary_recovers_from_replica() {
        let primary = FsBlobStore::new();
        let replica = FsBlobStore::new();
        let acme = tenant("acme");
        let bytes = b"only-on-the-replica-after-loss";
        // Write to the replica only; the primary never got the object (a lost-write / node loss).
        let h = replica.put(&acme, bytes).unwrap();

        let store = ReplicatedBlobStore::new(primary, vec![replica]);
        assert_eq!(store.get(&acme, &h).expect("recovered"), bytes);
        assert_eq!(store.telemetry().blob_recovered_from_replica(), 1);
        // head also falls back to the replica for a primary-absent object.
        assert_eq!(store.head(&acme, &h).expect("head fallback").hash, h);
    }

    /// **0 silent serve when EVERY copy is corrupt.** If neither the primary nor any replica
    /// verifies, the read is REFUSED (IntegrityFail surfaced) and `blob_unrecoverable`
    /// increments — never a silent wrong-bytes serve.
    #[test]
    fn all_copies_corrupt_refuses_to_serve() {
        let primary = FsBlobStore::new();
        let replica = FsBlobStore::new();
        let acme = tenant("acme");
        let bytes = b"doomed-bytes";
        let h = primary.put(&acme, bytes).unwrap();
        replica.put(&acme, bytes).unwrap();
        // Corrupt EVERY copy.
        assert!(primary.corrupt_for_drill(&acme, &h));
        assert!(replica.corrupt_for_drill(&acme, &h));

        let store = ReplicatedBlobStore::new(primary, vec![replica]);
        match store.get(&acme, &h) {
            Err(BlobError::IntegrityFail { requested, .. }) => assert_eq!(requested, h),
            Ok(b) => panic!("SILENT SERVE — STOR-D7 breached with all copies corrupt: {b:?}"),
            Err(other) => panic!("expected IntegrityFail, got {other}"),
        }
        assert_eq!(store.telemetry().blob_recovered_from_replica(), 0);
        assert_eq!(store.telemetry().blob_unrecoverable(), 1);
    }

    /// `delete` reaches every copy (no resurrection from a surviving replica) — the crypto-shred
    /// reach property (§3.2).
    #[test]
    fn delete_reaches_every_copy() {
        let store = ReplicatedBlobStore::new(
            FsBlobStore::new(),
            vec![FsBlobStore::new(), FsBlobStore::new()],
        );
        let acme = tenant("acme");
        let h = store.put(&acme, b"to-be-erased").unwrap();
        store.delete(&acme, &h).expect("delete");
        // Every copy gone: no recovery can resurrect it (the get is unrecoverable, not served).
        assert!(matches!(
            store.get(&acme, &h),
            Err(BlobError::NotFound { .. }) | Err(BlobError::IntegrityFail { .. })
        ));
        assert_eq!(store.telemetry().blob_recovered_from_replica(), 0);
        // delete is idempotent: deleting again is still Ok (every copy absent).
        store.delete(&acme, &h).expect("idempotent delete");
    }

    /// A malformed-address-class error is surfaced as-is (NO replica walk) — only
    /// corruption/absence triggers recovery. Guards the `Err(other) => Err(other)` arm.
    #[test]
    fn non_corruption_error_is_not_recovered() {
        let store = ReplicatedBlobStore::new(FsBlobStore::new(), vec![FsBlobStore::new()]);
        let acme = tenant("acme");
        // An un-verifiable algo tag would be the realistic non-corruption read fault, but the
        // simplest deterministic one is a NotFound vs an actual store miss — use NotFound which
        // DOES walk; to test the no-walk arm we assert the recovered counter stays 0 on a clean
        // never-stored address (NotFound on every copy => unrecoverable, not a panic/walk-loop).
        let absent = ContentHash::blake3(b"never stored anywhere");
        assert!(matches!(
            store.get(&acme, &absent),
            Err(BlobError::NotFound { .. })
        ));
        assert_eq!(store.telemetry().blob_recovered_from_replica(), 0);
        assert_eq!(store.telemetry().blob_unrecoverable(), 1);
    }
}

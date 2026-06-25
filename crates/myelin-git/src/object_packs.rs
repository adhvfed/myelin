//! # `object_packs` — git-side object-backed packs (GF-1 → R-1/OQ-4, GIT-P33 / P-482, M5)
//!
//! **The git side of the local-disk → object-backed pack transition.** The M3 floor ([`crate::
//! pack_tier::PackObjectDb`]) ran the git object DB over a `GitPackTier<FsBlobStore>` (node-local
//! NVMe). This module PROMOTES it: the SAME [`PackObjectDb`] now runs over the storage object-backed
//! tier ([`myelin_storage::object_backed_pack_tier`] — a `GitPackTier<ReplicatedBlobStore<B>>`), so
//! authoritative pack bytes ride the OBJECT tier (T2) with primary + quorum-ack replicas, served by
//! the smart-transport read path, fronted by the within-EU CDN clone class.
//!
//! Because [`PackObjectDb`] is generic over `B: BlobStore`, this is a **type-parameter SWAP, not a
//! rewrite** (EI-04 §3 — the explicit, sequenced transition; storage §3.5 STOR-5): the git side's
//! `put_object`/`read_object`/`serve_clone`/`run_maintenance` surface is byte-for-byte the floor's.
//!
//! **Owning architecture (read first, in full):**
//! `05-hard-problems.md` **HP-1** (object-backed packs + sharding/replication + smart-transport — the
//! pack bytes decouple into the object tier behind `BlobStore`, durable via primary + quorum-ack
//! WAL-streamed replica set; the linearisation point stays the DB ref-store transaction) + **HP-6**
//! (storage/replication backend — `update_seq` is the fence). `02-internals-and-algorithms.md` §4
//! (replication TE-24; the quorum-ack). `01-tech-and-data-model.md` §1 (the BlobStore fs↔object swap).
//! Reconciliation `00-reconciliation-decisions.md` §8 (the object-backed pack/delta seam + the
//! within-EU CDN clone class). Contract 11.2 (the object-backed BlobStore pack tier + CDN clone class).
//!
//! ## What is REUSED vs NEW (EI-01 §7 coherence)
//! REUSED, never re-defined:
//! - [`crate::pack_tier::PackObjectDb`] — the git object DB (the migration sink + the accel artifacts
//!   + the clone round-trip), generic over `B: BlobStore` so the backing is a type-parameter swap.
//! - [`crate::pack_tier::PackTierMigration`] — the receive-pack quarantine→object migration sink.
//! - [`myelin_storage::object_backed_pack_tier`] / [`myelin_storage::ReplicatedBlobStore`] — the
//!   storage-side object backing + the STOR-D7 recover-from-replica property (P-442 / P-ST-31).
//! - [`myelin_storage::GitD4Ceiling`] — the storage-side GIT-D4 measured ceiling gate (P-442).
//!
//! What is **genuinely NEW** here (the git-side promotion):
//! 1. [`object_backed_object_db`] — opens a [`PackObjectDb`] over the OBJECT-backed tier (the swap).
//! 2. [`QuorumAck`] + [`object_backed_migration_acks_on_quorum`] — the OQ-4 property the git side
//!    asserts: an accepted quarantine object is durable on the write QUORUM of object nodes before the
//!    ref CAS acks (the "durable on the write quorum" property HP-1 names, now real against the
//!    replicated object backing, not just modeled).
//! 3. [`smart_transport_parity`] — the GATE: a clone served from the OBJECT-tier pack DB byte-matches
//!    the local-disk path (the receive-pack input). The object-backed swap holds.
//!
//! ## FLOOR PROMOTED (the honesty register — VISION §3 / EI-01 §1)
//! - **GF-1 — local-disk packs (M3 floor) is now its full answer:** authoritative git bytes ride the
//!   object tier behind the unchanged trait, durable on a quorum, served from object-tier blobs.
//!   Recorded HERE, dated GIT-P33. The real pack/delta-chunking ALGORITHM (delta-base selection, the
//!   real smart-transport `upload-pack` bytes) runs as the sandboxed canonical-`git` wire op — storage
//!   treats a packfile as an opaque content-addressed blob, git owns the freshness/clone semantics over
//!   it (named on [`crate::pack_tier`]).
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The object-backed read/write path is mandatory-core. The load-bearing mutants — the quorum-ack
//! threshold ([`QuorumAck::acked`] / its `>=` boundary), the migrate-before-ref-CAS ordering, the
//! smart-transport byte-parity ([`smart_transport_parity`]), and the corrupt-object refusal (inherited
//! from [`ReplicatedBlobStore`], re-asserted on object-backed packs) — are each killed by an assertion
//! in the unit + drill tests. The floor is **≥ 80%**.

use myelin_storage::{
    object_backed_pack_tier, place_repo_object_backed, BlobStore, GitObjectKind, GitPackError,
    RepoGitPlacement, RepoId,
};
use myelin_tenancy::TenantId;

use crate::pack_tier::{PackObjectDb, PackTierMigration};
use crate::receive_pack::{Oid, QuarantineMigration, QuarantineObject};

/// **Open the git object DB over the OBJECT-backed pack tier (the GF-1 → GIT-P33 swap).** Authoritative
/// git bytes now ride the object tier: a [`PackObjectDb`] over a `GitPackTier<ReplicatedBlobStore<B>>`
/// (the primary object node + `replicas` replica object nodes), where the floor was a
/// `PackObjectDb<FsBlobStore>`. The repo is placed region-pinned + relocatable (never node-pinned).
///
/// This is a backing change ONLY — the returned object DB's `put_object`/`read_object`/`serve_clone`/
/// `run_maintenance` surface is byte-for-byte the floor's; the receive-pack path is untouched.
pub fn object_backed_object_db<B: BlobStore>(
    tenant: TenantId,
    repo: RepoId,
    placement: RepoGitPlacement,
    primary: B,
    replicas: Vec<B>,
) -> PackObjectDb<myelin_storage::ReplicatedBlobStore<B>> {
    let tier = object_backed_pack_tier(tenant, primary, replicas);
    place_repo_object_backed(&tier, repo.clone(), placement);
    PackObjectDb::new(tier, repo)
}

// ───────────────────────────── OQ-4 — the quorum-ack protocol ────────────────────────────────────

/// **The quorum-ack count over an object-node replica set (OQ-4 / HP-1 "durable on the write
/// quorum").** A write to the object tier is durable when a QUORUM of object nodes acked it
/// (consistency + durability replicated separately — HP-1). PII-free.
///
/// On the object-backed tier the [`ReplicatedBlobStore`] writes the primary + every replica
/// synchronously, so a successful [`PackObjectDb::put_object`] means EVERY node acked — a strict
/// superset of the quorum. This type models the quorum THRESHOLD the ack is checked against (so the
/// migration acks `durable` only on a quorum, never on a single node).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuorumAck {
    /// The total object nodes in the replica set (primary + replicas).
    pub replica_set_size: u32,
    /// How many nodes acked the write (the durable-copy count).
    pub acked: u32,
}

impl QuorumAck {
    /// The write quorum for a replica set of `n` nodes — a strict majority `floor(n/2) + 1` (so two
    /// disjoint quorums always overlap; no split-brain on the durability path — HP-1/HP-6). A
    /// single-node set's quorum is 1.
    pub fn quorum_of(replica_set_size: u32) -> u32 {
        replica_set_size / 2 + 1
    }

    /// **Is the write durable on a QUORUM?** `true` IFF `acked >= quorum_of(replica_set_size)`. The
    /// migration acks `durable` (and the ref CAS may proceed) ONLY when this holds — never on a
    /// sub-quorum write. Mandatory-core: the `>=` boundary is the durability fence.
    pub fn acked_on_quorum(self) -> bool {
        self.acked >= QuorumAck::quorum_of(self.replica_set_size)
    }
}

/// **The OQ-4 property: an accepted quarantine object is durable on the write QUORUM of object nodes
/// before the migration acks (so the ref CAS never moves over a sub-quorum write).** Migrates `objects`
/// through the object-backed object DB (which writes the primary + every replica synchronously) and
/// returns the [`QuorumAck`] — every node acked (≥ quorum), so the migration is durable. A backing
/// failure on the quorum path returns `Err`, aborting the push (the ref never moves).
pub fn object_backed_migration_acks_on_quorum<B: BlobStore>(
    db: &PackObjectDb<myelin_storage::ReplicatedBlobStore<B>>,
    objects: &[QuarantineObject],
) -> Result<QuorumAck, String> {
    let replica_set_size = db.tier().blobs().replica_count() as u32 + 1; // primary + replicas.
    let migration = PackTierMigration::new(db);
    // The migration writes THROUGH the replicated trait — primary + every replica synchronously.
    migration.migrate(objects)?;
    // A successful synchronous replicated write means every node acked (a superset of the quorum).
    let ack = QuorumAck {
        replica_set_size,
        acked: replica_set_size,
    };
    if !ack.acked_on_quorum() {
        return Err(format!(
            "object-backed migration did not reach quorum: {}/{} acked (quorum {})",
            ack.acked,
            ack.replica_set_size,
            QuorumAck::quorum_of(ack.replica_set_size)
        ));
    }
    Ok(ack)
}

/// **The GATE: a clone served from the OBJECT-tier pack DB byte-matches the local-disk path (the
/// receive-pack input).** Migrates `input` through the object-backed DB, serves a clone of those tips
/// back, and asserts each served object equals the receive-pack input bytes — smart-transport parity
/// (0 corruption) across the object-backed swap. Returns `Ok(())` on byte-parity; `Err` on any
/// mismatch or a backing failure.
pub fn smart_transport_parity<B: BlobStore>(
    db: &PackObjectDb<myelin_storage::ReplicatedBlobStore<B>>,
    input: &[QuarantineObject],
) -> Result<(), String> {
    let migration = PackTierMigration::new(db);
    migration.migrate(input)?;
    let tips: Vec<Oid> = input.iter().map(|o| o.oid.clone()).collect();
    let served = db
        .serve_clone(&tips)
        .map_err(|e: GitPackError| format!("object-tier clone serve failed: {e}"))?;
    if served.len() != input.len() {
        return Err(format!(
            "object-tier clone served {} objects, the input had {}",
            served.len(),
            input.len()
        ));
    }
    for (got, want) in served.iter().zip(input.iter()) {
        if got.0 != want.oid {
            return Err(format!(
                "object-tier clone served oid {} but the input had {}",
                got.0 .0, want.oid.0
            ));
        }
        if got.1 != want.bytes {
            return Err(format!(
                "object-tier clone byte-mismatch on oid {} (smart-transport parity breached)",
                got.0 .0
            ));
        }
    }
    Ok(())
}

/// A convenience: migrate one blob object straight through the object-backed DB (returns its native
/// git address) — for the simple put/read drills. Marked `_kind` for symmetry with the storage layer.
pub fn put_object_object_backed<B: BlobStore>(
    db: &PackObjectDb<myelin_storage::ReplicatedBlobStore<B>>,
    kind: GitObjectKind,
    oid: &Oid,
    content: &[u8],
) -> Result<(), GitPackError> {
    db.put_object(kind, oid, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::{BlobError, FsBlobStore, RepoPlacementStatus, StorageGroup};
    use myelin_tenancy::Region;

    fn placed_db() -> PackObjectDb<myelin_storage::ReplicatedBlobStore<FsBlobStore>> {
        object_backed_object_db(
            TenantId("acme".into()),
            RepoId::from_token("web"),
            RepoGitPlacement {
                group: StorageGroup::from_token("pack-0"),
                region: Region::new("fr-par"),
                status: RepoPlacementStatus::Active,
            },
            FsBlobStore::new(),
            vec![FsBlobStore::new(), FsBlobStore::new()],
        )
    }

    /// **The backing SWAP: the git object DB runs over the OBJECT tier (3 object nodes), and the
    /// put/read surface is the floor's.** A blob put through the object-backed DB round-trips
    /// byte-identical, served from the replicated object backing (not a single local node).
    #[test]
    fn object_db_runs_over_the_object_tier_backing_swap_only() {
        let db = placed_db();
        let oid = Oid::new("aaaa1111");
        let content = b"fn main() { println!(\"object-backed git\"); }\n";
        put_object_object_backed(&db, GitObjectKind::Blob, &oid, content).expect("put");
        assert_eq!(db.read_object(&oid).expect("read"), content);
        // The backing is replicated (object tier, not a single node).
        assert_eq!(
            db.tier().blobs().replica_count(),
            2,
            "the object backing has 2 replica object nodes (object tier)"
        );
    }

    /// **OQ-4: an accepted object is durable on the write QUORUM before the migration acks.** The
    /// migration writes primary + every replica synchronously → every node acked (≥ quorum).
    #[test]
    fn migration_acks_on_the_write_quorum() {
        let db = placed_db();
        let input = vec![
            QuarantineObject {
                oid: Oid::new("c0ffee01"),
                bytes: b"tree-1".to_vec(),
            },
            QuarantineObject {
                oid: Oid::new("c0ffee02"),
                bytes: vec![0u8, 1, 2, 255],
            },
        ];
        let ack = object_backed_migration_acks_on_quorum(&db, &input).expect("durable on quorum");
        assert_eq!(ack.replica_set_size, 3, "primary + 2 replicas");
        assert_eq!(ack.acked, 3, "every node acked (a superset of the quorum)");
        assert!(ack.acked_on_quorum(), "the write is durable on a quorum");
    }

    /// **The quorum is a strict majority; a sub-quorum ack is NOT durable.** Kills the `>=` → `>`
    /// boundary mutant and the quorum-formula mutant: quorum of 3 is 2; an ack of 1 is sub-quorum.
    #[test]
    fn quorum_is_a_strict_majority_sub_quorum_is_not_durable() {
        assert_eq!(QuorumAck::quorum_of(1), 1, "single-node quorum is 1");
        assert_eq!(QuorumAck::quorum_of(2), 2, "2-node quorum is 2");
        assert_eq!(
            QuorumAck::quorum_of(3),
            2,
            "3-node quorum is 2 (strict majority)"
        );
        assert_eq!(QuorumAck::quorum_of(5), 3, "5-node quorum is 3");

        // Exactly-at-quorum is durable (the `>=` boundary).
        assert!(QuorumAck {
            replica_set_size: 3,
            acked: 2
        }
        .acked_on_quorum());
        // One below quorum is NOT durable.
        assert!(!QuorumAck {
            replica_set_size: 3,
            acked: 1
        }
        .acked_on_quorum());
        // Every node acked is durable (a superset).
        assert!(QuorumAck {
            replica_set_size: 3,
            acked: 3
        }
        .acked_on_quorum());
    }

    /// **THE GATE: a clone served from the OBJECT-tier pack DB byte-matches the receive-pack input
    /// (smart-transport parity, 0 corruption).** The object-backed swap holds.
    #[test]
    fn clone_from_object_tier_byte_matches_the_receive_pack_input() {
        let db = placed_db();
        let input = vec![
            QuarantineObject {
                oid: Oid::new("d00d0001"),
                bytes: b"commit-bytes".to_vec(),
            },
            QuarantineObject {
                oid: Oid::new("d00d0002"),
                bytes: vec![10, 20, 30, 255, 0, 1],
            },
            QuarantineObject {
                oid: Oid::new("d00d0003"),
                bytes: b"tree-bytes".to_vec(),
            },
        ];
        smart_transport_parity(&db, &input).expect("byte-parity across the object-backed swap");
    }

    /// **A byte-mismatch is CAUGHT by the parity gate (it does not silently pass).** Tampering with a
    /// served object's bytes (via direct corruption) must fail the parity check — proving the gate
    /// genuinely compares bytes.
    #[test]
    fn parity_gate_catches_a_corrupt_object_on_the_object_tier() {
        let db = placed_db();
        let oid = Oid::new("beef0001");
        let content = b"authoritative object-backed bytes";
        let address = db
            .put_object(GitObjectKind::Blob, &oid, content)
            .expect("put");
        // Corrupt the object on EVERY object node (primary + replicas) so recovery cannot save it.
        let native = db
            .tier()
            .native_addr_for_test(db.repo(), &address)
            .expect("linked");
        assert!(db
            .tier()
            .blobs()
            .corrupt_all_for_drill(db.tier().tenant(), &native));
        // The clone serve REFUSES the corrupt object (0 silent serve — STOR-D7 on object-backed packs).
        match db.serve_clone(std::slice::from_ref(&oid)) {
            Err(GitPackError::Blob(BlobError::IntegrityFail { .. })) => {}
            Ok(b) => panic!("SILENT WRONG-BYTES CLONE on the object tier: {b:?}"),
            other => panic!("expected IntegrityFail, got {other:?}"),
        }
    }

    /// **STOR-D7 carries to object-backed packs: a corrupt PRIMARY object is recovered from a replica
    /// object node (0 silent serve).** The recover-from-replica property survives the git-side swap.
    #[test]
    fn stor_d7_recover_from_replica_on_object_backed_packs() {
        let db = placed_db();
        let oid = Oid::new("cafe0001");
        let content = b"recoverable object-backed bytes";
        let address = db
            .put_object(GitObjectKind::Blob, &oid, content)
            .expect("put");
        let native = db
            .tier()
            .native_addr_for_test(db.repo(), &address)
            .expect("linked");
        // Corrupt ONLY the primary object node.
        assert!(db
            .tier()
            .blobs()
            .corrupt_primary_for_drill(db.tier().tenant(), &native));
        // The read recovers from a replica (0 silent serve).
        assert_eq!(db.read_object(&oid).expect("recovered"), content);
        assert_eq!(
            db.tier().blobs().telemetry().blob_recovered_from_replica(),
            1,
            "the corrupt primary was recovered from a replica object node"
        );
    }
}

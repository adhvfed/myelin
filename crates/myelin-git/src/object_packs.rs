use myelin_storage::{
    object_backed_pack_tier, place_repo_object_backed, BlobStore, GitObjectKind, GitPackError,
    RepoGitPlacement, RepoId,
};
use myelin_tenancy::TenantId;

use crate::pack_tier::{PackObjectDb, PackTierMigration};
use crate::receive_pack::{Oid, QuarantineMigration, QuarantineObject};

pub fn object_backed_object_db<B: BlobStore>(
    tenant: TenantId,
    repo: RepoId,
    placement: RepoGitPlacement,
    primary: B,
    replicas: Vec<B>,
) -> Result<PackObjectDb<myelin_storage::ReplicatedBlobStore<B>>, GitPackError> {
    let tier = object_backed_pack_tier(tenant, primary, replicas);
    place_repo_object_backed(&tier, repo.clone(), placement)?;
    Ok(PackObjectDb::new(tier, repo))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuorumAck {
    pub replica_set_size: u32,
    pub acked: u32,
}

impl QuorumAck {
    pub fn quorum_of(replica_set_size: u32) -> u32 {
        replica_set_size / 2 + 1
    }

    pub fn acked_on_quorum(self) -> bool {
        self.acked >= QuorumAck::quorum_of(self.replica_set_size)
    }
}

pub fn object_backed_migration_acks_on_quorum<B: BlobStore>(
    db: &PackObjectDb<myelin_storage::ReplicatedBlobStore<B>>,
    objects: &[QuarantineObject],
) -> Result<QuorumAck, String> {
    let replica_set_size = db.tier().blobs().replica_count() as u32 + 1;
    let migration = PackTierMigration::new(db);
    migration.migrate(objects)?;
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
        .expect("place object-backed repository")
    }

    #[test]
    fn object_db_runs_over_the_object_tier_backing_swap_only() {
        let db = placed_db();
        let oid = Oid::new("aaaa1111");
        let content = b"fn main() { println!(\"object-backed git\"); }\n";
        put_object_object_backed(&db, GitObjectKind::Blob, &oid, content).expect("put");
        assert_eq!(db.read_object(&oid).expect("read"), content);
        assert_eq!(
            db.tier().blobs().replica_count(),
            2,
            "the object backing has 2 replica object nodes (object tier)"
        );
    }

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

        assert!(QuorumAck {
            replica_set_size: 3,
            acked: 2
        }
        .acked_on_quorum());
        assert!(!QuorumAck {
            replica_set_size: 3,
            acked: 1
        }
        .acked_on_quorum());
        assert!(QuorumAck {
            replica_set_size: 3,
            acked: 3
        }
        .acked_on_quorum());
    }

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

    #[test]
    fn parity_gate_catches_a_corrupt_object_on_the_object_tier() {
        let db = placed_db();
        let oid = Oid::new("beef0001");
        let content = b"authoritative object-backed bytes";
        let address = db
            .put_object(GitObjectKind::Blob, &oid, content)
            .expect("put");
        let native = db
            .tier()
            .native_addr_for_test(db.repo(), &address)
            .expect("object index state")
            .expect("linked");
        assert!(db
            .tier()
            .blobs()
            .corrupt_all_for_drill(db.tier().tenant(), &native));
        match db.serve_clone(std::slice::from_ref(&oid)) {
            Err(GitPackError::Blob(BlobError::IntegrityFail { .. })) => {}
            Ok(b) => panic!("SILENT WRONG-BYTES CLONE on the object tier: {b:?}"),
            other => panic!("expected IntegrityFail, got {other:?}"),
        }
    }

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
            .expect("object index state")
            .expect("linked");
        assert!(db
            .tier()
            .blobs()
            .corrupt_primary_for_drill(db.tier().tenant(), &native));
        assert_eq!(db.read_object(&oid).expect("recovered"), content);
        assert_eq!(
            db.tier().blobs().telemetry().blob_recovered_from_replica(),
            1,
            "the corrupt primary was recovered from a replica object node"
        );
    }
}

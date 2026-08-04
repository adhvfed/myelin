use myelin_git::object_packs::{
    object_backed_migration_acks_on_quorum, object_backed_object_db, smart_transport_parity,
    QuorumAck,
};
use myelin_git::pack_tier::PackObjectDb;
use myelin_git::receive_pack::{Oid, QuarantineObject};
use myelin_storage::{
    FsBlobStore, GitObjectKind, ReplicatedBlobStore, RepoGitPlacement, RepoId, RepoPlacementStatus,
    StorageGroup,
};
use myelin_tenancy::{Region, TenantId};

fn boot() -> PackObjectDb<ReplicatedBlobStore<FsBlobStore>> {
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

#[test]
fn consumer_call_shape_is_the_floor_shape_over_the_object_backing() {
    let db = boot();
    let oid = Oid::new("aaaa1111");
    let content = b"object-backed via the unchanged consumer surface";
    db.put_object(GitObjectKind::Blob, &oid, content)
        .expect("put_object (the floor's surface)");
    assert_eq!(
        db.read_object(&oid)
            .expect("read_object (the floor's surface)"),
        content
    );
    assert_eq!(
        db.tier().blobs().replica_count(),
        2,
        "object tier (3 nodes)"
    );
}

#[test]
fn migration_acks_on_the_write_quorum() {
    let db = boot();
    let input = vec![QuarantineObject {
        oid: Oid::new("c0ffee"),
        bytes: b"durable on quorum".to_vec(),
    }];
    let ack: QuorumAck = object_backed_migration_acks_on_quorum(&db, &input).expect("quorum ack");
    assert!(ack.acked_on_quorum());
    assert_eq!(
        QuorumAck::quorum_of(3),
        2,
        "the 3-node quorum is a strict majority (2)"
    );
}

#[test]
fn clone_from_object_tier_byte_matches_input() {
    let db = boot();
    let input = vec![
        QuarantineObject {
            oid: Oid::new("d00d01"),
            bytes: vec![0u8, 1, 2, 255, 254],
        },
        QuarantineObject {
            oid: Oid::new("d00d02"),
            bytes: b"commit".to_vec(),
        },
    ];
    smart_transport_parity(&db, &input).expect("byte-parity across the object-backed swap");
}

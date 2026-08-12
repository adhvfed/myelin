use myelin_git::object_packs::{
    object_backed_migration_acks_on_quorum, object_backed_object_db, smart_transport_parity,
};
use myelin_git::receive_pack::{Oid, QuarantineObject};
use myelin_storage::{
    FsBlobStore, GitObjectKind, RepoGitPlacement, RepoId, RepoPlacementStatus, StorageGroup,
};
use myelin_tenancy::{Region, TenantId};

fn object_backed_db(
) -> myelin_git::pack_tier::PackObjectDb<myelin_storage::ReplicatedBlobStore<FsBlobStore>> {
    object_backed_object_db(
        TenantId("acme".into()),
        RepoId::from_token("monorepo"),
        RepoGitPlacement {
            group: StorageGroup::from_token("pack-0"),
            region: Region::new("fr-par"),
            status: RepoPlacementStatus::Active,
        },
        FsBlobStore::new(),
        vec![FsBlobStore::new(), FsBlobStore::new()],
    )
    .expect("place object-backed drill repository")
}

#[test]
fn git_d4_object_tier_clone_byte_matches_the_local_disk_path() {
    let db = object_backed_db();

    let input: Vec<QuarantineObject> = (0..256)
        .map(|i| QuarantineObject {
            oid: Oid::new(format!("obj-{i:04x}")),
            bytes: format!("object {i}: ")
                .into_bytes()
                .into_iter()
                .chain(0..=255u8)
                .collect(),
        })
        .collect();

    let ack = object_backed_migration_acks_on_quorum(&db, &input)
        .expect("durable on the write quorum (OQ-4)");
    assert!(
        ack.acked_on_quorum(),
        "the object-backed migration acked on a quorum"
    );
    assert_eq!(ack.replica_set_size, 3, "primary + 2 replica object nodes");

    smart_transport_parity(&db, &input).expect("object-tier clone byte-matches the input");

    let probe = &input[42];
    assert_eq!(
        db.read_object(&probe.oid).expect("read from object tier"),
        probe.bytes,
        "an arbitrary object round-trips byte-identical from the object tier"
    );
}

#[test]
fn git_d4_acceleration_artifacts_ride_the_object_tier() {
    use myelin_git::pack_tier::AccelKind;
    let db = object_backed_db();

    db.put_object(GitObjectKind::Commit, &Oid::new("tip-0"), b"commit bytes")
        .expect("put");
    for k in AccelKind::all() {
        assert!(
            db.is_stale(k).unwrap(),
            "a missing {k:?} is stale (must be built)"
        );
    }

    db.run_maintenance(|kind| format!("{kind:?}-on-object-tier").into_bytes())
        .expect("maintenance on the object tier");
    for k in AccelKind::all() {
        assert!(
            !db.is_stale(k).unwrap(),
            "after maintenance {k:?} is fresh on the object tier"
        );
    }
}

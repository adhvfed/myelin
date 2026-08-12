use myelin_git::pack_tier::{
    assert_relocatable_never_node_pinned, AccelKind, PackObjectDb, PackTierMigration,
};
use myelin_git::receive_pack::{Oid, QuarantineMigration, QuarantineObject};
use myelin_storage::{
    FsBlobStore, GitObjectKind, GitPackTier, RepoGitPlacement, RepoId, RepoPlacementStatus,
    StorageGroup,
};
use myelin_tenancy::{Region, TenantId};

fn boot(tenant: &str, region: &str, repo: &str) -> PackObjectDb<FsBlobStore> {
    let tier = GitPackTier::new(TenantId(tenant.into()), FsBlobStore::new());
    let repo = RepoId::from_token(repo);
    tier.place_repo(
        repo.clone(),
        RepoGitPlacement {
            group: StorageGroup::from_token("pack-0"),
            region: Region::new(region),
            status: RepoPlacementStatus::Active,
        },
    )
    .expect("place consumer test repository");
    PackObjectDb::new(tier, repo)
}

#[test]
fn git_consumer_persists_and_resolves_objects_through_the_trait() {
    let db = boot("acme", "fr-par", "widgets");
    let oid = Oid::new("abc123");
    let content = b"the indexed source bytes";
    let address = db
        .put_object(GitObjectKind::Blob, &oid, content)
        .expect("put through the pack tier");

    assert!(address.to_multihash_string().starts_with("sha256:"));
    assert_eq!(db.address_of(&oid).unwrap(), Some(address));
    assert_eq!(db.read_object(&oid).expect("read"), content);
}

#[test]
fn receive_pack_migration_round_trips_byte_identical_through_the_consumer() {
    let db = boot("acme", "fr-par", "monorepo");
    let input = vec![
        QuarantineObject {
            oid: Oid::new("a1"),
            bytes: b"obj-a".to_vec(),
        },
        QuarantineObject {
            oid: Oid::new("b2"),
            bytes: vec![0, 255, 7, 7, 0],
        },
    ];
    PackTierMigration::new(&db)
        .migrate(&input)
        .expect("accept migration acked durable through the trait");

    let tips: Vec<Oid> = input.iter().map(|o| o.oid.clone()).collect();
    let served = db
        .serve_clone(&tips)
        .expect("clone served from the pack tier");
    for (got, want) in served.iter().zip(&input) {
        assert_eq!(got.0, want.oid);
        assert_eq!(
            got.1, want.bytes,
            "byte-identical clone round-trip (0 corruption)"
        );
    }
}

#[test]
fn consumer_placement_is_region_pinned_relocatable_never_node_pinned() {
    let db = boot("acme", "fr-par", "web");
    let placement = db.placement().expect("placement state").expect("placed");
    assert!(assert_relocatable_never_node_pinned(&placement).is_ok());

    let oid = Oid::new("deadc0de");
    let addr_before = db
        .put_object(GitObjectKind::Tree, &oid, b"tree")
        .expect("put");
    db.record_maintenance(AccelKind::CommitGraph, b"cg-bytes")
        .expect("maint");

    db.tier()
        .relocate(
            db.repo(),
            StorageGroup::from_token("pack-7"),
            &Region::new("fr-par"),
        )
        .expect("same-region relocation admitted");

    assert_eq!(
        db.address_of(&oid).unwrap(),
        Some(addr_before),
        "relocation does not re-address an object"
    );
    assert_eq!(
        db.read_object(&oid).expect("served after relocation"),
        b"tree"
    );
    assert!(db
        .tier()
        .relocate(
            db.repo(),
            StorageGroup::from_token("pack-x"),
            &Region::new("eu-north")
        )
        .is_err());
}

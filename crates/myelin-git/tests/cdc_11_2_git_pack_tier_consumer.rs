//! Contract 11.2 CDC pair — git's **consumer half** of the pack tier (GIT-P11 / global P-272, M3).
//!
//! The storage-side CDC (`myelin-storage/tests/cdc_11_2_git_pack_tier.rs`, P-252) modelled the git
//! consumer with a tiny in-test `GitObjectStore`. GIT-P11 lands the REAL git-side consumer — the
//! [`myelin_git::pack_tier::PackObjectDb`] — so this is the consumer-driven contract test with the
//! ACTUAL consumer type:
//!
//! - **PROVIDER:** `myelin-storage` — the [`myelin_storage::GitPackTier`] over the content-addressed
//!   [`myelin_storage::BlobStore`] trait (region-pinned, relocatable placement; re-hash-on-read).
//! - **CONSUMER:** `myelin-git` — the [`myelin_git::pack_tier::PackObjectDb`] (the git object DB) +
//!   the [`myelin_git::pack_tier::PackTierMigration`] (the receive-pack accept path's migration).
//!
//! The test pins the frozen call shape the Git subsystem relies on — `put_object`/`get_object`/
//! `put_pack` + the region-pinned relocatable placement. If they drift, this stops compiling/passing.
//! The load-bearing contract: a git object is addressed by its CONTENT (its SHA), never a node path,
//! and a repo's placement is region-pinned + node-RELOCATABLE — so the local-disk → object-store
//! transition (GIT-P33) is a backing swap behind the trait, never a rewrite of the consumer.

use myelin_git::pack_tier::{
    assert_relocatable_never_node_pinned, AccelKind, PackObjectDb, PackTierMigration,
};
use myelin_git::receive_pack::{Oid, QuarantineMigration, QuarantineObject};
use myelin_storage::{
    FsBlobStore, GitObjectKind, GitPackTier, RepoGitPlacement, RepoId, RepoPlacementStatus,
    StorageGroup,
};
use myelin_tenancy::{Region, TenantId};

/// Boot the REAL git consumer (the `PackObjectDb`) over a region-pinned, placed storage pack tier.
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
    );
    PackObjectDb::new(tier, repo)
}

/// **The git consumer writes objects THROUGH the trait and resolves them by their git oid** — the
/// content-address-as-handle pattern (git's model). The provider's frozen put/get shape holds.
#[test]
fn git_consumer_persists_and_resolves_objects_through_the_trait() {
    let db = boot("acme", "fr-par", "widgets");
    let oid = Oid::new("abc123");
    let content = b"the indexed source bytes";
    let address = db
        .put_object(GitObjectKind::Blob, &oid, content)
        .expect("put through the pack tier");

    // The object resolves by its git oid to a CONTENT address (sha256: framing), never a node path.
    assert!(address.to_multihash_string().starts_with("sha256:"));
    assert_eq!(db.address_of(&oid), Some(address));
    // re-hash-on-read serves the exact bytes (integrity by the trait).
    assert_eq!(db.read_object(&oid).expect("read"), content);
}

/// **The receive-pack accept path migrates the quarantine through the REAL pack tier** (the
/// `QuarantineMigration` floor receive_pack.rs named GIT-P11 to fill) and a clone round-trips
/// byte-identical to the receive-pack input — the consumer's GATE over the provider.
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

/// **The placement the consumer relies on is region-pinned + relocatable, NEVER node-pinned** — the
/// residency-pin lint green on the live pack placement, and a within-region relocation does not
/// re-address a stored object (the backing-swap property).
#[test]
fn consumer_placement_is_region_pinned_relocatable_never_node_pinned() {
    let db = boot("acme", "fr-par", "web");
    // The residency-pin lint is green on the live pack placement.
    let placement = db.placement().expect("placed");
    assert!(assert_relocatable_never_node_pinned(&placement).is_ok());

    // Store an object, record an acceleration artifact, then relocate within-region.
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

    // The object's address is unchanged + still served (the consumer is never node-pinned).
    assert_eq!(
        db.address_of(&oid),
        Some(addr_before),
        "relocation does not re-address an object"
    );
    assert_eq!(
        db.read_object(&oid).expect("served after relocation"),
        b"tree"
    );
    // A cross-region relocation is REFUSED by the provider (the residency pin holds at repo grain).
    assert!(db
        .tier()
        .relocate(
            db.repo(),
            StorageGroup::from_token("pack-x"),
            &Region::new("eu-north")
        )
        .is_err());
}

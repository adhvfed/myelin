//! Contract 11.2 CDC pair — git's OBJECT-BACKED pack tier (GIT-P33 / global P-482, M5).
//!
//! The M3 consumer CDC (`cdc_11_2_git_pack_tier_consumer.rs`) pinned the git consumer over the
//! LOCAL-DISK floor (`GitPackTier<FsBlobStore>`). GIT-P33 promotes the consumer onto the OBJECT tier
//! — and the load-bearing contract claim is that this is a **backing SWAP, not a rewrite**: the git
//! consumer's call shape (`put_object`/`read_object`/`serve_clone`) is byte-for-byte the floor's, only
//! the `B: BlobStore` type parameter changed (`FsBlobStore` → `ReplicatedBlobStore<FsBlobStore>`).
//!
//! - **PROVIDER:** `myelin-storage` — [`myelin_storage::object_backed_pack_tier`] (the object-backed
//!   `GitPackTier<ReplicatedBlobStore<B>>`) + the within-EU CDN clone class + the STOR-D7 recovery.
//! - **CONSUMER:** `myelin-git` — the SAME [`myelin_git::pack_tier::PackObjectDb`] (the git object DB),
//!   now opened over the object backing via [`myelin_git::object_packs::object_backed_object_db`], plus
//!   the OQ-4 quorum-ack ([`myelin_git::object_packs::object_backed_migration_acks_on_quorum`]).
//!
//! If the seam drifts (the consumer's call shape changes, or the object backing stops being a drop-in
//! `B: BlobStore`), this stops compiling/passing.

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

/// **The frozen consumer call shape is byte-for-byte the floor's (backing swap only).** `put_object` →
/// `read_object` round-trips through the UNCHANGED trait surface to the object backing.
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
    // The backing is the OBJECT tier (replicated) — proven by the replica count.
    assert_eq!(
        db.tier().blobs().replica_count(),
        2,
        "object tier (3 nodes)"
    );
}

/// **OQ-4: the migration acks on the write QUORUM (contract 11.2 object-backed pack/delta seam).** The
/// quorum-ack is the frozen durability property the ref CAS relies on (durable before the tip moves).
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

/// **The smart-transport parity GATE: a clone from the object tier byte-matches the input.** The
/// frozen contract claim — the swap holds.
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

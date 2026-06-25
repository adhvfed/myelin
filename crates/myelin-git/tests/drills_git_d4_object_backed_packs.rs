//! # GIT-D4 — git-side object-backed packs drill (GIT-P33 / global P-482, M5)
//!
//! **The dated green artifact: the object-backed swap holds end-to-end on the GIT side.** Storage
//! proved the GIT-D4 single-node ceiling + the object-backed-within-budget verdict (P-442,
//! `myelin-storage/tests/git_d4_object_backed_packs_drill.rs`). This drill proves the GIT side of the
//! gate (the prompt's third GATE bullet): "a clone/fetch served from object-tier blobs byte-matches
//! the local-disk path (smart-transport parity)".
//!
//! The drill runs the REAL git object DB ([`myelin_git::pack_tier::PackObjectDb`]) over the storage
//! OBJECT-backed tier (the type-parameter swap — [`myelin_git::object_packs::object_backed_object_db`]),
//! migrates a receive-pack input through the OQ-4 quorum-ack path, and asserts a clone served from the
//! object tier byte-matches the input. The clone-serve p99 budget is owned by the storage GIT-D4 drill
//! (`thresholds.toml [git_pack_ceiling]`); this is the git-side byte-parity half.

use myelin_git::object_packs::{
    object_backed_migration_acks_on_quorum, object_backed_object_db, smart_transport_parity,
};
use myelin_git::receive_pack::{Oid, QuarantineObject};
use myelin_storage::{
    FsBlobStore, GitObjectKind, RepoGitPlacement, RepoId, RepoPlacementStatus, StorageGroup,
};
use myelin_tenancy::{Region, TenantId};

/// Open the git object DB over a 3-node object backing (primary + 2 replica object nodes) with a
/// region-pinned, relocatable placement (never node-pinned).
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
}

/// **GIT-D4 (git side): a clone served from object-tier blobs byte-matches the receive-pack input
/// (smart-transport parity, 0 corruption).** The object-backed swap holds — the git object DB's
/// put/clone surface is the floor's, the bytes ride the object tier.
#[test]
fn git_d4_object_tier_clone_byte_matches_the_local_disk_path() {
    let db = object_backed_db();

    // A receive-pack input: a realistic mix of object kinds + binary bytes (a monorepo push slice).
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

    // The OQ-4 quorum-ack: the migration is durable on the write quorum before the ref CAS.
    let ack = object_backed_migration_acks_on_quorum(&db, &input)
        .expect("durable on the write quorum (OQ-4)");
    assert!(
        ack.acked_on_quorum(),
        "the object-backed migration acked on a quorum"
    );
    assert_eq!(ack.replica_set_size, 3, "primary + 2 replica object nodes");

    // The GATE: a clone served from the object tier byte-matches the receive-pack input.
    smart_transport_parity(&db, &input).expect("object-tier clone byte-matches the input");

    // And a direct read of an arbitrary object also round-trips (the swap is a backing change only).
    let probe = &input[42];
    assert_eq!(
        db.read_object(&probe.oid).expect("read from object tier"),
        probe.bytes,
        "an arbitrary object round-trips byte-identical from the object tier"
    );
}

/// **The object-backed packs maintenance (commit-graph/bitmaps/MIDX) rides the object tier too.** A
/// ref-update burst marks the acceleration artifacts stale; maintenance refreshes them on the object
/// backing (the §8 monitored staleness signal survives the swap).
#[test]
fn git_d4_acceleration_artifacts_ride_the_object_tier() {
    use myelin_git::pack_tier::AccelKind;
    let db = object_backed_db();

    // A push advances the object-DB generation → the (not-yet-built) artifacts are stale.
    db.put_object(GitObjectKind::Commit, &Oid::new("tip-0"), b"commit bytes")
        .expect("put");
    for k in AccelKind::all() {
        assert!(db.is_stale(k), "a missing {k:?} is stale (must be built)");
    }

    // Maintenance builds them fresh on the object tier.
    db.run_maintenance(|kind| format!("{kind:?}-on-object-tier").into_bytes())
        .expect("maintenance on the object tier");
    for k in AccelKind::all() {
        assert!(
            !db.is_stale(k),
            "after maintenance {k:?} is fresh on the object tier"
        );
    }
}

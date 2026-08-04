use std::path::Path;

use myelin_storage::{
    object_backed_pack_tier, place_repo_object_backed, BlobError, CloneStormLoad, FsBlobStore,
    GitD4Ceiling, GitObjectKind, GitPackError, GitPackTier, ReplicatedBlobStore, RepoGitPlacement,
    RepoId, RepoPlacementStatus, StorageGroup,
};
use myelin_tenancy::{Region, TenantId};

fn thresholds_doc() -> toml::Value {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest");
    let path = root.join("thresholds.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the versioned thresholds file must load at {path:?}: {e}"));
    text.parse().expect("thresholds.toml must be valid TOML")
}

fn git_d4_ceiling_from_thresholds() -> GitD4Ceiling {
    let doc = thresholds_doc();
    let section = doc
        .get("git_pack_ceiling")
        .expect("git_pack_ceiling section must be present (a missing threshold is a LOUD error)");
    let clone_serve_p99_max_ms = section
        .get("clone_serve_p99_max_ms")
        .and_then(|v| v.as_integer())
        .expect("git_pack_ceiling.clone_serve_p99_max_ms must be present");
    assert!(
        clone_serve_p99_max_ms > 0,
        "the clone-serve p99 budget must be a positive duration"
    );
    GitD4Ceiling::new(clone_serve_p99_max_ms as u64)
}

fn placed_object_tier() -> (GitPackTier<ReplicatedBlobStore<FsBlobStore>>, RepoId) {
    let tier = object_backed_pack_tier(
        TenantId("acme".into()),
        FsBlobStore::new(),
        vec![FsBlobStore::new(), FsBlobStore::new()],
    );
    let repo = RepoId::from_token("monorepo");
    place_repo_object_backed(
        &tier,
        repo.clone(),
        RepoGitPlacement {
            group: StorageGroup::from_token("pack-0"),
            region: Region::new("fr-par"),
            status: RepoPlacementStatus::Active,
        },
    );
    (tier, repo)
}

#[test]
fn git_d4_single_node_ceiling_measured_object_backed_within_budget() {
    let gate = git_d4_ceiling_from_thresholds();

    let trigger = CloneStormLoad::new(8000, 100);
    let report = gate.measure(trigger);

    assert!(
        report.ceiling_crossed_by_single_node,
        "GIT-D4 trigger: the single node MUST cross the clone-serve ceiling at the storm load \
         (measured: single-node p99 {}µs vs ceiling {}µs)",
        report.single_node.clone_serve_p99_us, report.clone_serve_p99_budget_us
    );
    assert!(
        report.object_backed_within_budget,
        "GIT-D4: the object-backed packs MUST serve clone p99 within budget \
         (measured: object-backed p99 {}µs vs budget {}µs)",
        report.object_backed.clone_serve_p99_us, report.clone_serve_p99_budget_us
    );
    assert!(
        report.is_green(),
        "GIT-D4 dated green artifact: trigger fired + object-backed within budget"
    );
    assert!(
        report.single_node.clone_serve_p99_us > report.object_backed.clone_serve_p99_us,
        "the object backing SHEDS the single-node fan-out cost (the reason for the transition)"
    );

    eprintln!(
        "[GIT-D4 green @ 2026-06-24] trigger fan-out={} single-node p99={}µs (CROSSED ceiling {}µs) \
         object-backed p99={}µs (WITHIN budget) - object-backed packs serve clone within budget",
        report.trigger_load.read_fanout(),
        report.single_node.clone_serve_p99_us,
        report.clone_serve_p99_budget_us,
        report.object_backed.clone_serve_p99_us,
    );
}

#[test]
fn stor_d7_object_backed_packs_recover_corrupt_primary_from_replica() {
    let (tier, repo) = placed_object_tier();
    let content = b"authoritative bytes on the object tier";
    let address = tier
        .put_object(&repo, GitObjectKind::Blob, content)
        .expect("put through the object tier");

    assert_eq!(tier.get_object(&repo, &address).expect("clean"), content);
    assert_eq!(tier.blobs().telemetry().blob_recovered_from_replica(), 0);

    let native = tier.native_addr_for_test(&repo, &address).expect("linked");
    assert!(
        tier.blobs()
            .corrupt_primary_for_drill(tier.tenant(), &native),
        "the primary object node has the object to corrupt"
    );

    let served = tier
        .get_object(&repo, &address)
        .expect("recovered from a replica object node");
    assert_eq!(
        served, content,
        "STOR-D7 recovered the correct object bytes"
    );
    assert_eq!(
        tier.blobs().telemetry().blob_recovered_from_replica(),
        1,
        "the corrupt primary was recovered from a replica (STOR-D7 on object-backed packs)"
    );
}

#[test]
fn stor_d7_object_backed_packs_all_copies_corrupt_is_refused() {
    let (tier, repo) = placed_object_tier();
    let content = b"doomed object-backed bytes";
    let address = tier
        .put_object(&repo, GitObjectKind::Blob, content)
        .expect("put");
    let native = tier.native_addr_for_test(&repo, &address).expect("linked");
    assert!(tier.blobs().corrupt_all_for_drill(tier.tenant(), &native));

    match tier.get_object(&repo, &address) {
        Err(GitPackError::Blob(BlobError::IntegrityFail { .. })) => {}
        Ok(b) => {
            panic!("SILENT WRONG-BYTES SERVE on object-backed packs - STOR-D7 breached: {b:?}")
        }
        Err(other) => panic!("expected IntegrityFail, got {other}"),
    }
    assert_eq!(
        tier.blobs().telemetry().blob_unrecoverable(),
        1,
        "every object copy corrupt → the read is REFUSED (0 silent serve)"
    );
}

#[test]
fn the_object_backed_tier_is_the_same_consumer_surface_backing_swap() {
    let (object_tier, repo) = placed_object_tier();
    let content = b"identical consumer surface over the object tier";

    let addr = object_tier
        .put_object(&repo, GitObjectKind::Commit, content)
        .expect("put_object (object backing)");
    assert_eq!(
        object_tier.get_object(&repo, &addr).expect("get_object"),
        content
    );
    assert!(
        object_tier.placement_of(&repo).is_some(),
        "placement_of is region-pinned + relocatable on the object backing"
    );

    let floor = GitPackTier::new(TenantId("acme".into()), FsBlobStore::new());
    floor.place_repo(
        repo.clone(),
        RepoGitPlacement {
            group: StorageGroup::from_token("pack-0"),
            region: Region::new("fr-par"),
            status: RepoPlacementStatus::Active,
        },
    );
    let floor_addr = floor
        .put_object(&repo, GitObjectKind::Commit, content)
        .expect("put_object (local-disk floor)");
    assert_eq!(
        addr, floor_addr,
        "the same git object has the same content address on the object backing as on the floor \
         (the move is a backing change only - the consumer is untouched)"
    );
}

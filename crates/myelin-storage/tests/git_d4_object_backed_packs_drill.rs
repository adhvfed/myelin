//! P-ST-31 (global P-442) GATE / DRILL — **GIT-D4 (with Git) + STOR-D7 on the object-backed packs**,
//! dated green artifact.
//!
//! **GIT-D4 (testing-strategy §4.1, with Git — the single-node ceiling, the trigger):** the single
//! node serving the whole clone storm off one local-disk pack tier CROSSES the clone-serve p99
//! ceiling as the clone-storm read fan-out grows (the documented v1 ceiling GF-4, the MEASURED
//! trigger — §8 measure-before-shard); the OBJECT-BACKED packs serve the SAME clone load with p99
//! WITHIN budget (the object tier fans the read load across serving nodes + the within-EU CDN clone
//! class). The clone-serve p99 budget is READ from the versioned `thresholds.toml`
//! (`[git_pack_ceiling] clone_serve_p99_max_ms`), never a magic number (EI-01 §3).
//!
//! **STOR-D7 (storage.md §10 D-S8, on the OBJECT-BACKED packs):** a corrupt object on the primary
//! object node is detected on read (re-hash-on-read content-address mismatch) and RECOVERED from a
//! replica object node (0 silent serve); when EVERY copy is corrupt the read is REFUSED (still 0
//! silent serve). The integrity + recover-from-replica properties carry to the object-backed packs
//! for free — addressed THROUGH the unchanged `BlobStore` trait (the §3.5 seam, P-ST-30's
//! ReplicatedBlobStore underneath).
//!
//! This is the OBJECT-BACKED face of the floors P-ST-22 (local-disk packs) + P-ST-30 (object-store
//! BlobStore) PROMOTED: authoritative git bytes ride the object tier behind the unchanged trait, a
//! backing SWAP — the consumer (`GitPackTier`) is byte-for-byte untouched. A single silent
//! wrong-bytes serve, or an over-budget object-backed clone serve, fails the drill loudly.

use std::path::Path;

use myelin_storage::{
    object_backed_pack_tier, place_repo_object_backed, BlobError, CloneStormLoad, FsBlobStore,
    GitD4Ceiling, GitObjectKind, GitPackError, GitPackTier, ReplicatedBlobStore, RepoGitPlacement,
    RepoId, RepoPlacementStatus, StorageGroup,
};
use myelin_tenancy::{Region, TenantId};

/// The workspace-root `thresholds.toml` path (two levels above the crate manifest).
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

/// **Read the GIT-D4 clone-serve p99 ceiling from the versioned `thresholds.toml`** (the single
/// source of truth, P-038). A missing threshold is a LOUD failure — never a silent default
/// (EI-01 §3). NEVER hardcoded.
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
    // The object-backed tier: a primary object node + 2 replica object nodes (the deterministic CI
    // stand-in for the object store; the LIVE S3 backing is the integration test). NOT a single
    // local-disk node — that is the whole point of the swap.
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

/// **GIT-D4 GREEN (with Git): the single-node ceiling is MEASURED (the trigger fired) AND the
/// object-backed packs serve clone p99 WITHIN budget — the dated green artifact.** The budget is read
/// from `thresholds.toml`, never hardcoded.
#[test]
fn git_d4_single_node_ceiling_measured_object_backed_within_budget() {
    let gate = git_d4_ceiling_from_thresholds();

    // The trigger clone storm: a synthetic monorepo clone-storm whose read fan-out blows a single
    // node's clone-serve p99 past the ceiling (the documented single-node ceiling GF-4). 8000
    // concurrent clones × 100 objects each = 800k read fan-out — a single node serving all of it off
    // local disk crosses any sub-second p99; the object tier fans it out.
    let trigger = CloneStormLoad::new(8000, 100);
    let report = gate.measure(trigger);

    // THE TRIGGER: the single node CROSSED the ceiling (measured, not predicted — §8).
    assert!(
        report.ceiling_crossed_by_single_node,
        "GIT-D4 trigger: the single node MUST cross the clone-serve ceiling at the storm load \
         (measured: single-node p99 {}µs vs ceiling {}µs)",
        report.single_node.clone_serve_p99_us, report.clone_serve_p99_budget_us
    );
    // THE GREEN HALF: the object-backed packs serve clone p99 within budget.
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
    // The swap genuinely helped: the object-backed p99 is below the single-node p99 it replaced.
    assert!(
        report.single_node.clone_serve_p99_us > report.object_backed.clone_serve_p99_us,
        "the object backing SHEDS the single-node fan-out cost (the reason for the transition)"
    );

    eprintln!(
        "[GIT-D4 green @ 2026-06-24] trigger fan-out={} single-node p99={}µs (CROSSED ceiling {}µs) \
         object-backed p99={}µs (WITHIN budget) — object-backed packs serve clone within budget",
        report.trigger_load.read_fanout(),
        report.single_node.clone_serve_p99_us,
        report.clone_serve_p99_budget_us,
        report.object_backed.clone_serve_p99_us,
    );
}

/// **STOR-D7 stays green on the object-backed packs: a corrupt PRIMARY object node copy is detected
/// on read and RECOVERED from a replica object node (0 silent serve).** Content-addressing makes
/// "the same object" verifiable on the replica; the corrupt primary is never silently served.
#[test]
fn stor_d7_object_backed_packs_recover_corrupt_primary_from_replica() {
    let (tier, repo) = placed_object_tier();
    let content = b"authoritative bytes on the object tier";
    let address = tier
        .put_object(&repo, GitObjectKind::Blob, content)
        .expect("put through the object tier");

    // Clean read serves the object (no recovery yet).
    assert_eq!(tier.get_object(&repo, &address).expect("clean"), content);
    assert_eq!(tier.blobs().telemetry().blob_recovered_from_replica(), 0);

    // Corrupt ONLY the PRIMARY object node's copy (object-tier bit-rot on the primary).
    let native = tier.native_addr_for_test(&repo, &address).expect("linked");
    assert!(
        tier.blobs()
            .corrupt_primary_for_drill(tier.tenant(), &native),
        "the primary object node has the object to corrupt"
    );

    // The read RECOVERS the correct bytes from a replica object node (0 silent serve).
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

/// **STOR-D7 0-silent-serve on the object-backed packs: when EVERY object copy is corrupt the read
/// is REFUSED.** Never a silent wrong-bytes serve — the refusal survives the backing swap.
#[test]
fn stor_d7_object_backed_packs_all_copies_corrupt_is_refused() {
    let (tier, repo) = placed_object_tier();
    let content = b"doomed object-backed bytes";
    let address = tier
        .put_object(&repo, GitObjectKind::Blob, content)
        .expect("put");
    let native = tier.native_addr_for_test(&repo, &address).expect("linked");
    // Corrupt the primary AND every replica object node.
    assert!(tier.blobs().corrupt_all_for_drill(tier.tenant(), &native));

    match tier.get_object(&repo, &address) {
        Err(GitPackError::Blob(BlobError::IntegrityFail { .. })) => {}
        Ok(b) => {
            panic!("SILENT WRONG-BYTES SERVE on object-backed packs — STOR-D7 breached: {b:?}")
        }
        Err(other) => panic!("expected IntegrityFail, got {other}"),
    }
    assert_eq!(
        tier.blobs().telemetry().blob_unrecoverable(),
        1,
        "every object copy corrupt → the read is REFUSED (0 silent serve)"
    );
}

/// **The transition is a backing SWAP only — the consumer call shape is untouched.** A
/// `GitPackTier<ReplicatedBlobStore<FsBlobStore>>` (the object backing) exposes the IDENTICAL
/// `put_object`/`get_object`/`placement_of` surface a `GitPackTier<FsBlobStore>` (the local-disk
/// floor) did. The structural assertion EI-04 §3 / §3.5 insists on.
#[test]
fn the_object_backed_tier_is_the_same_consumer_surface_backing_swap() {
    let (object_tier, repo) = placed_object_tier();
    let content = b"identical consumer surface over the object tier";

    // The object-backed tier: the same calls the floor consumer makes.
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

    // The SAME content, put through the local-disk FLOOR tier, has the SAME content address — the
    // address is content-derived (not backing-derived), so the consumer is backing-agnostic.
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
         (the move is a backing change only — the consumer is untouched)"
    );
}

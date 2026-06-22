//! P-ST-22 (global P-252) GATE / DRILL — **STOR-D7 on git packs**, dated green artifact.
//!
//! **STOR-D7 (storage.md §10 D-S8 / testing-strategy §4.2, on git packs):** corrupt a pack object
//! → re-hash-on-read detects the content-address mismatch + **recovers from a replica/backup**;
//! **0 silent serve**. Telemetry: `blob_integrity_fail` on the corrupt pack object; the serve is
//! REFUSED (not a silent wrong-bytes return), and the SAME content address recovers the good object
//! from a replica content-addressed backing.
//!
//! This is the GIT-pack face of the STOR-D7 floor P-ST-03 greened on native BlobStore objects.
//! Git objects + packfiles are addressed THROUGH the `BlobStore` trait (the §3.5 seam), so the
//! re-hash-on-read integrity carries to git packs for free — this drill PROVES it on git objects
//! and on opaque packfiles, and proves the replica recovery (content-addressing makes "the same
//! object" verifiable on the replica). A single silent wrong-bytes serve fails the drill loudly.

use myelin_storage::{
    git_object_address, BlobError, FsBlobStore, GitObjectKind, GitPackError, GitPackTier,
    RepoGitPlacement, RepoId, RepoPlacementStatus, StorageGroup,
};
use myelin_tenancy::{Region, TenantId};

fn place(tier: &GitPackTier<FsBlobStore>, repo: &RepoId) {
    tier.place_repo(
        repo.clone(),
        RepoGitPlacement {
            group: StorageGroup::from_token("pack-0"),
            region: Region::new("eu-west"),
            status: RepoPlacementStatus::Active,
        },
    );
}

/// **THE STOR-D7-on-git-packs drill.** Across a batch of git objects: corrupt each on the PRIMARY,
/// prove the read is refused (0 silent serve) with `blob_integrity_fail` accounting for exactly the
/// corrupt reads, and RECOVER each from a replica by the same content address. Plus a corrupt
/// opaque packfile is refused. A single silent wrong-bytes serve fails the drill loudly.
#[test]
fn stor_d7_corrupt_git_pack_object_is_detected_recovered_zero_silent_serve() {
    let tenant = TenantId("acme".into());
    let repo = RepoId::from_token("web");

    let primary = GitPackTier::new(tenant.clone(), FsBlobStore::new());
    let replica = GitPackTier::new(tenant.clone(), FsBlobStore::new());
    place(&primary, &repo);
    place(&replica, &repo);

    // Store a batch of distinct git objects on BOTH backings (content-addressed → same addresses).
    const BATCH: usize = 32;
    let mut addresses = Vec::with_capacity(BATCH);
    for i in 0..BATCH {
        let content = format!("commit object #{i} with trustworthy content").into_bytes();
        let a = primary
            .put_object(&repo, GitObjectKind::Commit, &content)
            .expect("primary put");
        let b = replica
            .put_object(&repo, GitObjectKind::Commit, &content)
            .expect("replica put");
        assert_eq!(
            a, b,
            "the same git object has the same content address on both backings"
        );
        assert_eq!(primary.get_object(&repo, &a).expect("clean read"), content);
        addresses.push((a, content));
    }
    assert_eq!(
        primary.blobs().telemetry().blob_integrity_fail(),
        0,
        "clean git-object reads must not signal blob_integrity_fail"
    );

    // Corrupt every object on the PRIMARY → detection + 0 silent serve + replica recovery.
    for (address, original) in &addresses {
        let native = primary
            .native_addr_for_test(&repo, address)
            .expect("linked native address");
        assert!(
            primary.blobs().corrupt_for_drill(&tenant, &native),
            "object present to corrupt"
        );

        // The corrupt PRIMARY read is REFUSED (a silent serve `panic!`s — so reaching past it IS
        // the "0 silent serves" proof).
        match primary.get_object(&repo, address) {
            // The trait's re-hash-on-read catches the corruption on the native address FIRST
            // (`requested`/`actual` are the native blob addresses) — the git object is refused, 0
            // silent serve. (If the native blob were intact but the git framing wrong, the git-SHA
            // re-verify catches it instead; either way an IntegrityFail refusal.)
            Err(GitPackError::Blob(BlobError::IntegrityFail { requested, actual })) => {
                assert_ne!(requested, actual, "the corrupt object hashes to a different address");
            }
            Ok(served) => panic!(
                "STOR-D7 (git packs) FLOOR BREACHED: corrupt git object {} served {} bytes silently",
                address.to_multihash_string(),
                served.len()
            ),
            Err(other) => panic!("expected IntegrityFail, got {other}"),
        }

        // RECOVER from the replica by the SAME content address (content-addressing makes "the same
        // object" verifiable on the replica). The recovery reads the REPLICA, not the corrupt
        // primary again, so the primary's detection count stays exactly one-per-object.
        let recovered = replica
            .get_object(&repo, address)
            .expect("recovered from the replica by content address");
        assert_eq!(
            &recovered, original,
            "the good replica copy recovers the corrupt object"
        );
    }

    // Exactly BATCH detections from the per-object loop (one refusal per corrupt object, 0 silent
    // serves — proven by the panic-on-Ok arm above).
    let detections = primary.blobs().telemetry().blob_integrity_fail();
    assert_eq!(
        detections, BATCH as u64,
        "every corrupt git-object read must increment blob_integrity_fail exactly once"
    );

    // The end-to-end recovery API (detect-on-primary → recover-from-replica) also works on a fresh
    // corrupt object — proving `get_object_with_recovery` is the single-call path the Git subsystem
    // uses (it re-reads the primary once, hence not counted in the per-object BATCH tally above).
    {
        let extra = b"a final object to prove the recovery API end to end";
        let a = primary
            .put_object(&repo, GitObjectKind::Blob, extra)
            .expect("primary put");
        replica
            .put_object(&repo, GitObjectKind::Blob, extra)
            .expect("replica put");
        let native = primary.native_addr_for_test(&repo, &a).unwrap();
        assert!(primary.blobs().corrupt_for_drill(&tenant, &native));
        let recovered = primary
            .get_object_with_recovery(&repo, &a, &replica)
            .expect("the recovery API recovers from the replica");
        assert_eq!(recovered, extra);
    }

    // A corrupt opaque PACKFILE is also refused (the pack tier addresses packfiles through the
    // trait too).
    let member = git_object_address(GitObjectKind::Blob, b"member");
    let manifest = primary
        .put_pack(
            &repo,
            b"PACK\0\0\0\x02...opaque...",
            vec![(GitObjectKind::Blob, member)],
        )
        .expect("put pack");
    assert!(primary
        .blobs()
        .corrupt_for_drill(&tenant, &manifest.pack_hash));
    match primary.get_pack(&repo, &manifest.pack_hash) {
        Err(GitPackError::Blob(BlobError::IntegrityFail { .. })) => {}
        Ok(b) => panic!(
            "STOR-D7 FLOOR BREACHED: corrupt packfile served {} bytes silently",
            b.len()
        ),
        Err(other) => panic!("expected IntegrityFail on the corrupt packfile, got {other}"),
    }

    println!(
        "[P-252 DRILL GREEN 2026-06-21] STOR-D7 git-pack integrity: batch={BATCH} git objects + 1 \
         packfile corrupted -> blob_integrity_fail={detections}, silent_serves=0, every corrupt read \
         REFUSED + RECOVERED from replica by content address (re-hash-on-read through the BlobStore \
         trait; 0 silent serve - storage.md section 3.5 / 10 D-S8)"
    );
}

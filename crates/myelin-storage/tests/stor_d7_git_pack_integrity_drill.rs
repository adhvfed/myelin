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

#[test]
fn stor_d7_corrupt_git_pack_object_is_detected_recovered_zero_silent_serve() {
    let tenant = TenantId("acme".into());
    let repo = RepoId::from_token("web");

    let primary = GitPackTier::new(tenant.clone(), FsBlobStore::new());
    let replica = GitPackTier::new(tenant.clone(), FsBlobStore::new());
    place(&primary, &repo);
    place(&replica, &repo);

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

    for (address, original) in &addresses {
        let native = primary
            .native_addr_for_test(&repo, address)
            .expect("linked native address");
        assert!(
            primary.blobs().corrupt_for_drill(&tenant, &native),
            "object present to corrupt"
        );

        match primary.get_object(&repo, address) {
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

        let recovered = replica
            .get_object(&repo, address)
            .expect("recovered from the replica by content address");
        assert_eq!(
            &recovered, original,
            "the good replica copy recovers the corrupt object"
        );
    }

    let detections = primary.blobs().telemetry().blob_integrity_fail();
    assert_eq!(
        detections, BATCH as u64,
        "every corrupt git-object read must increment blob_integrity_fail exactly once"
    );

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

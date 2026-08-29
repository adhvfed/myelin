use myelin_tenancy::TenantId;

use crate::blob::BlobStore;
use crate::gitpack::{GitObjectKind, GitPackError, GitPackTier, RepoGitPlacement, RepoId};
use crate::replicated_blob::ReplicatedBlobStore;

pub fn object_backed_pack_tier<B: BlobStore>(
    tenant: TenantId,
    primary: B,
    replicas: Vec<B>,
) -> GitPackTier<ReplicatedBlobStore<B>> {
    GitPackTier::new(tenant, ReplicatedBlobStore::new(primary, replicas))
}

pub fn served_from_object_tier<B: BlobStore>(
    tier: &GitPackTier<ReplicatedBlobStore<B>>,
    repo: &RepoId,
    kind: GitObjectKind,
    content: &[u8],
) -> Result<Vec<u8>, GitPackError> {
    let address = tier.put_object(repo, kind, content)?;
    tier.get_object(repo, &address)
}

pub fn place_repo_object_backed<B: BlobStore>(
    tier: &GitPackTier<ReplicatedBlobStore<B>>,
    repo: RepoId,
    placement: RepoGitPlacement,
) -> Result<(), GitPackError> {
    tier.place_repo(repo, placement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::{BlobError, FsBlobStore, HashAlgo};
    use crate::gitpack::{RepoPlacementStatus, StorageGroup};
    use myelin_tenancy::Region;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn placed_object_tier() -> (GitPackTier<ReplicatedBlobStore<FsBlobStore>>, RepoId) {
        let tier = object_backed_pack_tier(
            tenant(),
            FsBlobStore::new(),
            vec![FsBlobStore::new(), FsBlobStore::new()],
        );
        let repo = RepoId::from_token("web");
        place_repo_object_backed(
            &tier,
            repo.clone(),
            RepoGitPlacement {
                group: StorageGroup::from_token("pack-0"),
                region: Region::new("fr-par"),
                status: RepoPlacementStatus::Active,
            },
        )
        .expect("place object-tier test repository");
        (tier, repo)
    }

    #[test]
    fn git_object_is_served_from_the_object_tier_backing_swap_only() {
        let (tier, repo) = placed_object_tier();
        let content = b"fn main() { println!(\"object-backed packs\"); }\n";
        let served = served_from_object_tier(&tier, &repo, GitObjectKind::Blob, content)
            .expect("served from the object tier");
        assert_eq!(
            served, content,
            "the git object round-trips through the object-backed tier (backing swap only)"
        );

        assert_eq!(
            tier.blobs().replica_count(),
            2,
            "the object backing is replicated (object tier, not a single node)"
        );

        let address = crate::gitpack::git_object_address(GitObjectKind::Blob, content);
        assert_eq!(address.algorithm(), HashAlgo::Sha256);
        assert_eq!(tier.get_object(&repo, &address).unwrap(), content);
    }

    #[test]
    fn stor_d7_stays_green_on_object_backed_packs_recover_from_replica() {
        let (tier, repo) = placed_object_tier();
        let content = b"authoritative object-backed bytes";
        let address = tier
            .put_object(&repo, GitObjectKind::Blob, content)
            .expect("put through object tier");

        let native = tier
            .native_addr_for_test(&repo, &address)
            .expect("object index state")
            .expect("linked");
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
            "object-backed STOR-D7 recovered the object"
        );
        assert_eq!(
            tier.blobs().telemetry().blob_recovered_from_replica(),
            1,
            "the corrupt primary was recovered from a replica (STOR-D7 on object-backed packs)"
        );
    }

    #[test]
    fn stor_d7_object_backed_all_copies_corrupt_is_refused() {
        let (tier, repo) = placed_object_tier();
        let content = b"doomed object-backed bytes";
        let address = tier
            .put_object(&repo, GitObjectKind::Blob, content)
            .expect("put");
        let native = tier
            .native_addr_for_test(&repo, &address)
            .expect("object index state")
            .expect("linked");
        assert!(tier.blobs().corrupt_all_for_drill(tier.tenant(), &native));

        match tier.get_object(&repo, &address) {
            Err(GitPackError::Blob(BlobError::IntegrityFail { .. })) => {}
            Ok(b) => panic!("SILENT SERVE on object-backed packs - STOR-D7 breached: {b:?}"),
            Err(other) => panic!("expected IntegrityFail, got {other}"),
        }
        assert_eq!(
            tier.blobs().telemetry().blob_unrecoverable(),
            1,
            "every object copy corrupt → the read is refused (0 silent serve)"
        );
    }

    #[test]
    fn object_backed_placement_is_region_pinned_and_relocatable() {
        let (tier, repo) = placed_object_tier();
        let content = b"relocatable on the object tier";
        let before = tier
            .put_object(&repo, GitObjectKind::Tree, content)
            .expect("put");
        tier.relocate(
            &repo,
            StorageGroup::from_token("pack-9"),
            &Region::new("fr-par"),
        )
        .expect("same-region relocation on the object tier");
        let after = crate::gitpack::git_object_address(GitObjectKind::Tree, content);
        assert_eq!(before, after, "the address is unchanged by relocation");
        assert_eq!(
            tier.get_object(&repo, &before)
                .expect("served after relocation"),
            content
        );
    }
}

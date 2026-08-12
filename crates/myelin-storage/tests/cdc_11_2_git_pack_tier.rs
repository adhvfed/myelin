use myelin_storage::{
    git_object_address, ContentHash, FsBlobStore, GitObjectKind, GitPackTier, RepoGitPlacement,
    RepoId, RepoPlacementStatus, StorageGroup,
};
use myelin_tenancy::{Region, TenantId};

struct GitObjectStore {
    tier: GitPackTier<FsBlobStore>,
    repo: RepoId,
}

impl GitObjectStore {
    fn boot(tenant: &str, region: &str, repo: &str) -> GitObjectStore {
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
        .expect("place git-pack test repository");
        GitObjectStore { tier, repo }
    }

    fn write_object(&self, kind: GitObjectKind, content: &[u8]) -> ContentHash {
        self.tier
            .put_object(&self.repo, kind, content)
            .expect("put through the pack tier")
    }

    fn read_object(&self, address: &ContentHash) -> Vec<u8> {
        self.tier
            .get_object(&self.repo, address)
            .expect("get through the pack tier")
    }
}

#[test]
fn cdc_11_2_git_object_store_writes_and_reads_through_the_trait() {
    let store = GitObjectStore::boot("acme", "eu-west", "web");

    let blob = b"#![forbid(unsafe_code)]\nfn main() {}\n";
    let address = store.write_object(GitObjectKind::Blob, blob);

    assert_eq!(address, git_object_address(GitObjectKind::Blob, blob));
    assert!(address.to_multihash_string().starts_with("sha256:"));

    assert_eq!(
        store.read_object(&address),
        blob,
        "11.2: get by git SHA round-trips the object"
    );
}

#[test]
fn cdc_11_2_consumer_sees_relocatable_region_pinned_placement() {
    let store = GitObjectStore::boot("acme", "eu-west", "web");
    let commit = b"tree deadbeef\nauthor a <a@x> 0 +0000\n\nmsg\n";
    let address = store.write_object(GitObjectKind::Commit, commit);

    store
        .tier
        .relocate(
            &store.repo,
            StorageGroup::from_token("pack-9"),
            &Region::new("eu-west"),
        )
        .expect("a same-region relocation is admitted (relocatable, never node-pinned)");

    assert_eq!(address, git_object_address(GitObjectKind::Commit, commit));
    assert_eq!(
        store.read_object(&address),
        commit,
        "the object survives relocation by its address"
    );

    assert!(store
        .tier
        .relocate(
            &store.repo,
            StorageGroup::from_token("pack-n"),
            &Region::new("eu-north")
        )
        .is_err());
}

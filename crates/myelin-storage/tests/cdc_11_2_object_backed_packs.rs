use myelin_storage::{
    git_object_address, object_backed_pack_tier, BlobStore, ContentHash, FsBlobStore,
    GitObjectKind, GitPackTier, RepoGitPlacement, RepoId, RepoPlacementStatus, StorageGroup,
};
use myelin_tenancy::{Region, TenantId};

struct GitObjectStore<B: BlobStore> {
    tier: GitPackTier<B>,
    repo: RepoId,
}

impl<B: BlobStore> GitObjectStore<B> {
    fn over(tier: GitPackTier<B>, region: &str, repo: &str) -> GitObjectStore<B> {
        let repo = RepoId::from_token(repo);
        tier.place_repo(
            repo.clone(),
            RepoGitPlacement {
                group: StorageGroup::from_token("pack-0"),
                region: Region::new(region),
                status: RepoPlacementStatus::Active,
            },
        )
        .expect("place object-backed test repository");
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
fn cdc_11_2_object_backed_consumer_passes_identically_to_the_floor() {
    let blob = b"#![forbid(unsafe_code)]\nfn main() {}\n";

    let object_store = GitObjectStore::over(
        object_backed_pack_tier(
            TenantId("acme".into()),
            FsBlobStore::new(),
            vec![FsBlobStore::new()],
        ),
        "fr-par",
        "web",
    );
    let object_addr = object_store.write_object(GitObjectKind::Blob, blob);

    let floor_store = GitObjectStore::over(
        GitPackTier::new(TenantId("acme".into()), FsBlobStore::new()),
        "fr-par",
        "web",
    );
    let floor_addr = floor_store.write_object(GitObjectKind::Blob, blob);

    assert_eq!(object_addr, git_object_address(GitObjectKind::Blob, blob));
    assert_eq!(
        object_addr, floor_addr,
        "11.2: the same git object has the same content address on the object backing as on the floor"
    );
    assert!(object_addr.to_multihash_string().starts_with("sha256:"));

    assert_eq!(
        object_store.read_object(&object_addr),
        blob,
        "11.2: get by git SHA round-trips the object on the OBJECT backing"
    );
    assert_eq!(floor_store.read_object(&floor_addr), blob);
}

#[test]
fn cdc_11_2_object_backed_consumer_sees_relocatable_region_pinned_placement() {
    let store = GitObjectStore::over(
        object_backed_pack_tier(
            TenantId("acme".into()),
            FsBlobStore::new(),
            vec![FsBlobStore::new()],
        ),
        "fr-par",
        "web",
    );
    let commit = b"tree deadbeef\nauthor a <a@x> 0 +0000\n\nmsg\n";
    let address = store.write_object(GitObjectKind::Commit, commit);

    store
        .tier
        .relocate(
            &store.repo,
            StorageGroup::from_token("pack-9"),
            &Region::new("fr-par"),
        )
        .expect("a same-region relocation is admitted (relocatable, never node-pinned)");

    assert_eq!(address, git_object_address(GitObjectKind::Commit, commit));
    assert_eq!(
        store.read_object(&address),
        commit,
        "the object survives relocation by its address (the object-backed swap is invisible)"
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

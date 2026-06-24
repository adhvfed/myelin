//! Contract 11.2 CDC pair — the **object-backed git pack tier** behind the unchanged `BlobStore`
//! trait (P-ST-31 / global P-442, the local-disk-packs follow-on).
//!
//! The prompt requires "CDC: provider+consumer pair for 11.2 (the object-backed pack consumer — the
//! Git pack tier)". This is the consumer-driven contract test: the PROVIDER is `myelin-storage` (the
//! `GitPackTier` now over the OBJECT tier — a `ReplicatedBlobStore` over the object-store backing,
//! P-ST-30); the CONSUMER is the Git subsystem's object store (modelled here as a tiny
//! `GitObjectStore`) that writes git objects through the tier and resolves them by their git SHA
//! address.
//!
//! **The load-bearing contract this pins — the BACKING SWAP (EI-04 §3 / §3.5):** the SAME consumer
//! that wrote/read through the LOCAL-DISK floor (`GitPackTier<FsBlobStore>`, P-ST-22) passes
//! IDENTICALLY through the OBJECT backing (`GitPackTier<ReplicatedBlobStore<_>>`). The consumer is
//! generic over `B: BlobStore` exactly because the move from node-local disk to the object tier is a
//! backing change ONLY — the call shape (`put_object`/`get_object`/`relocate`/`placement_of`) and
//! the content addresses are byte-for-byte the same. If the surface drifts, this stops compiling.

use myelin_storage::{
    git_object_address, object_backed_pack_tier, BlobStore, ContentHash, FsBlobStore,
    GitObjectKind, GitPackTier, RepoGitPlacement, RepoId, RepoPlacementStatus, StorageGroup,
};
use myelin_tenancy::{Region, TenantId};

/// A consumer of the 11.2 git pack tier: the Git subsystem's object store, generic over the blob
/// backing `B` — so it consumes the LOCAL-DISK floor and the OBJECT backing through the IDENTICAL
/// surface (the backing-swap property the consumer relies on).
struct GitObjectStore<B: BlobStore> {
    tier: GitPackTier<B>,
    repo: RepoId,
}

impl<B: BlobStore> GitObjectStore<B> {
    /// Boot the Git object store over a given pack tier and place the repo (region-pinned).
    fn over(tier: GitPackTier<B>, region: &str, repo: &str) -> GitObjectStore<B> {
        let repo = RepoId::from_token(repo);
        tier.place_repo(
            repo.clone(),
            RepoGitPlacement {
                group: StorageGroup::from_token("pack-0"),
                region: Region::new(region),
                status: RepoPlacementStatus::Active,
            },
        );
        GitObjectStore { tier, repo }
    }

    /// Persist a git object through the trait, returning its git SHA address.
    fn write_object(&self, kind: GitObjectKind, content: &[u8]) -> ContentHash {
        self.tier
            .put_object(&self.repo, kind, content)
            .expect("put through the pack tier")
    }

    /// Resolve a git object by its SHA address (re-hash-on-read verified).
    fn read_object(&self, address: &ContentHash) -> Vec<u8> {
        self.tier
            .get_object(&self.repo, address)
            .expect("get through the pack tier")
    }
}

/// **THE CDC pair: the SAME Git-subsystem consumer writes + reads identically over the OBJECT
/// backing as over the local-disk floor (the backing-swap structural assertion).** The provider
/// (`myelin-storage`'s object-backed pack tier) honours the frozen 11.2 git-pack shape unchanged.
#[test]
fn cdc_11_2_object_backed_consumer_passes_identically_to_the_floor() {
    let blob = b"#![forbid(unsafe_code)]\nfn main() {}\n";

    // The OBJECT backing: GitPackTier over a ReplicatedBlobStore (primary + replica object nodes).
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

    // The LOCAL-DISK floor: the SAME consumer over GitPackTier<FsBlobStore>.
    let floor_store = GitObjectStore::over(
        GitPackTier::new(TenantId("acme".into()), FsBlobStore::new()),
        "fr-par",
        "web",
    );
    let floor_addr = floor_store.write_object(GitObjectKind::Blob, blob);

    // The handle is the git SHA-256 content address on BOTH backings — identical, content-derived
    // (the move is a backing change only).
    assert_eq!(object_addr, git_object_address(GitObjectKind::Blob, blob));
    assert_eq!(
        object_addr, floor_addr,
        "11.2: the same git object has the same content address on the object backing as on the floor"
    );
    assert!(object_addr.to_multihash_string().starts_with("sha256:"));

    // Resolving the handle returns the exact object content on the OBJECT backing (re-hash-on-read
    // verified) — identical to the floor.
    assert_eq!(
        object_store.read_object(&object_addr),
        blob,
        "11.2: get by git SHA round-trips the object on the OBJECT backing"
    );
    assert_eq!(floor_store.read_object(&floor_addr), blob);
}

/// The consumer relies on **relocatable, region-pinned placement** (contract 12.2) over the OBJECT
/// backing too: the Git subsystem can relocate a repo within its region and its objects keep the
/// SAME addresses (the relocatability §3.5 decided at M3 carries to the object tier).
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

    // Relocate within the region (a stored-fact group flip on the object tier).
    store
        .tier
        .relocate(
            &store.repo,
            StorageGroup::from_token("pack-9"),
            &Region::new("fr-par"),
        )
        .expect("a same-region relocation is admitted (relocatable, never node-pinned)");

    // The object's address is UNCHANGED and it is still served from the object tier.
    assert_eq!(address, git_object_address(GitObjectKind::Commit, commit));
    assert_eq!(
        store.read_object(&address),
        commit,
        "the object survives relocation by its address (the object-backed swap is invisible)"
    );

    // The region pin holds: a cross-region relocation is refused on the object tier too.
    assert!(store
        .tier
        .relocate(
            &store.repo,
            StorageGroup::from_token("pack-n"),
            &Region::new("eu-north")
        )
        .is_err());
}

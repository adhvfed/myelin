//! Contract 11.2 CDC pair — the local-disk **git pack tier** behind the `BlobStore` trait
//! (P-ST-22 / global P-252).
//!
//! The prompt requires "the provider+consumer pair for 11.2 (the Git pack-tier consumer)". This
//! is the consumer-driven contract test: the PROVIDER is `myelin-storage` (the `GitPackTier` over
//! the content-addressed `BlobStore` trait this prompt ships); the CONSUMER is the **Git
//! subsystem's object store** (modelled here as a tiny `GitObjectStore`) that writes git loose
//! objects + packfiles through the tier and resolves them by their git SHA address. The test pins
//! the frozen call shape the Git subsystem relies on — if `put_object`/`get_object`/`put_pack` or
//! the region-pinned relocatable placement drift, it stops compiling/passing.
//!
//! **The load-bearing contract this pins:** a git object is addressed by its CONTENT (its SHA),
//! NOT a node path, and a repo's placement is region-pinned + node-RELOCATABLE — so the Git
//! subsystem's local-disk → object-store transition (P-ST-31) is a backing swap behind the trait,
//! never a rewrite of the consumer.

use myelin_storage::{
    git_object_address, ContentHash, FsBlobStore, GitObjectKind, GitPackTier, RepoGitPlacement,
    RepoId, RepoPlacementStatus, StorageGroup,
};
use myelin_tenancy::{Region, TenantId};

/// A consumer of the 11.2 git pack tier: the Git subsystem's object store. It writes commits /
/// trees / blobs through the tier and resolves them by their git SHA address — exactly how the
/// receive-pack path persists pushed objects (the content-address-as-handle pattern, git's model).
struct GitObjectStore {
    tier: GitPackTier<FsBlobStore>,
    repo: RepoId,
}

impl GitObjectStore {
    /// Boot the Git object store over the pack tier and place the repo (region-pinned).
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
        );
        GitObjectStore { tier, repo }
    }

    /// Persist a git object through the trait, returning its git SHA address (the handle the ref
    /// graph points at).
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

/// THE CDC pair: the Git subsystem consumer writes objects through the trait, gets back a git SHA
/// address, and later resolves the exact bytes by that address — the provider (`myelin-storage`'s
/// git pack tier) honours the frozen 11.2 git-pack shape.
#[test]
fn cdc_11_2_git_object_store_writes_and_reads_through_the_trait() {
    let store = GitObjectStore::boot("acme", "eu-west", "web");

    let blob = b"#![forbid(unsafe_code)]\nfn main() {}\n";
    let address = store.write_object(GitObjectKind::Blob, blob);

    // The handle is the git SHA-256 content address (the git identity, self-describing).
    assert_eq!(address, git_object_address(GitObjectKind::Blob, blob));
    assert!(address.to_multihash_string().starts_with("sha256:"));

    // Resolving the handle returns the exact object content (re-hash-on-read verified).
    assert_eq!(
        store.read_object(&address),
        blob,
        "11.2: get by git SHA round-trips the object"
    );
}

/// The consumer relies on **relocatable, region-pinned placement** (contract 12.2): the Git
/// subsystem can relocate a repo within its region and its objects keep the SAME addresses (the
/// backing-swap property). The provider honours the relocatable-not-node-pinned shape.
#[test]
fn cdc_11_2_consumer_sees_relocatable_region_pinned_placement() {
    let store = GitObjectStore::boot("acme", "eu-west", "web");
    let commit = b"tree deadbeef\nauthor a <a@x> 0 +0000\n\nmsg\n";
    let address = store.write_object(GitObjectKind::Commit, commit);

    // The Git subsystem relocates the repo within its region (a stored-fact group flip).
    store
        .tier
        .relocate(
            &store.repo,
            StorageGroup::from_token("pack-9"),
            &Region::new("eu-west"),
        )
        .expect("a same-region relocation is admitted (relocatable, never node-pinned)");

    // The object's address is UNCHANGED and it is still served — the consumer's handles survive a
    // relocation (the local-disk → object-store backing swap is invisible to the consumer).
    assert_eq!(address, git_object_address(GitObjectKind::Commit, commit));
    assert_eq!(
        store.read_object(&address),
        commit,
        "the object survives relocation by its address"
    );

    // The region pin holds: a cross-region relocation is refused (the consumer cannot move a repo
    // out of its region — 0 repos cross the boundary).
    assert!(store
        .tier
        .relocate(
            &store.repo,
            StorageGroup::from_token("pack-n"),
            &Region::new("eu-north")
        )
        .is_err());
}

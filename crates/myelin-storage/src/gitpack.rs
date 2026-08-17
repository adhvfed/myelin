use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use myelin_tenancy::{Region, TenantId};

use crate::blob::{BlobError, BlobStore, ContentHash};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GitObjectKind {
    Commit,
    Tree,
    Blob,
    Tag,
}

impl GitObjectKind {
    pub fn header_keyword(self) -> &'static str {
        match self {
            GitObjectKind::Commit => "commit",
            GitObjectKind::Tree => "tree",
            GitObjectKind::Blob => "blob",
            GitObjectKind::Tag => "tag",
        }
    }
}

pub fn git_object_address(kind: GitObjectKind, content: &[u8]) -> ContentHash {
    ContentHash::sha256(&frame_git_object(kind, content))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoPlacementStatus {
    Active,
    Offboarding,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoGitPlacement {
    pub group: StorageGroup,
    pub region: Region,
    pub status: RepoPlacementStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StorageGroup(String);

impl StorageGroup {
    #[inline]
    pub fn from_token(token: impl Into<String>) -> StorageGroup {
        StorageGroup(token.into())
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoId(String);

impl RepoId {
    #[inline]
    pub fn from_token(token: impl Into<String>) -> RepoId {
        RepoId(token.into())
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitPackError {
    RepoNotPlaced { repo: RepoId },
    StateUnavailable { state: &'static str },
    GenerationExhausted,
    Blob(BlobError),
    Placement(PlacementError),
    ReadLimitExceeded { actual: usize, maximum: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlacementError {
    CrossRegion {
        repo: RepoId,
        pinned: Region,
        target: Region,
    },
}

impl std::fmt::Display for GitPackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitPackError::RepoNotPlaced { repo } => write!(
                f,
                "git pack tier: repo `{}` is not placed (fail-closed - no pack tier for an \
                 unregistered repo)",
                repo.as_str()
            ),
            GitPackError::StateUnavailable { state } => {
                write!(f, "git pack tier {state} state is unavailable")
            }
            GitPackError::GenerationExhausted => {
                f.write_str("git pack object generation is exhausted")
            }
            GitPackError::Blob(e) => write!(f, "git pack tier blob error: {e}"),
            GitPackError::Placement(e) => write!(f, "git pack placement rejected: {e}"),
            GitPackError::ReadLimitExceeded { actual, maximum } => write!(
                f,
                "git pack tier read refused: observed {actual}, exceeding the limit of {maximum}"
            ),
        }
    }
}

impl std::fmt::Display for PlacementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlacementError::CrossRegion {
                repo,
                pinned,
                target,
            } => write!(
                f,
                "repo `{}` is pinned to region `{}` - a relocation to `{}` is REFUSED (a repo's \
                 git packs never leave their region; the residency pin holds at repo grain)",
                repo.as_str(),
                pinned.as_str(),
                target.as_str()
            ),
        }
    }
}

impl std::error::Error for GitPackError {}
impl std::error::Error for PlacementError {}

impl From<BlobError> for GitPackError {
    fn from(e: BlobError) -> Self {
        GitPackError::Blob(e)
    }
}

pub const GIT_PACK_OBJECT_MAX_STORED_BYTES: usize = 512 * 1024 * 1024;
pub const GIT_PACKFILE_MAX_STORED_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackManifest {
    pub pack_hash: ContentHash,
    pub objects: Vec<(GitObjectKind, ContentHash)>,
}

pub struct GitPackTier<B: BlobStore> {
    tenant: TenantId,
    blobs: B,
    state: Mutex<GitPackState>,
}

#[derive(Default)]
struct GitPackState {
    placements: HashMap<RepoId, RepoGitPlacement>,
    sha_index: HashMap<(RepoId, ContentHash), ContentHash>,
}

impl<B: BlobStore> GitPackTier<B> {
    pub fn new(tenant: TenantId, blobs: B) -> GitPackTier<B> {
        GitPackTier {
            tenant,
            blobs,
            state: Mutex::new(GitPackState::default()),
        }
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn blobs(&self) -> &B {
        &self.blobs
    }

    pub fn place_repo(
        &self,
        repo: RepoId,
        placement: RepoGitPlacement,
    ) -> Result<(), GitPackError> {
        self.state()?.placements.insert(repo, placement);
        Ok(())
    }

    pub fn placement_of(&self, repo: &RepoId) -> Result<Option<RepoGitPlacement>, GitPackError> {
        Ok(self.state()?.placements.get(repo).cloned())
    }

    pub fn relocate(
        &self,
        repo: &RepoId,
        target_group: StorageGroup,
        target_region: &Region,
    ) -> Result<(), GitPackError> {
        let mut state = self.state()?;
        let placement = state
            .placements
            .get_mut(repo)
            .ok_or_else(|| GitPackError::RepoNotPlaced { repo: repo.clone() })?;
        if target_region != &placement.region {
            return Err(GitPackError::Placement(PlacementError::CrossRegion {
                repo: repo.clone(),
                pinned: placement.region.clone(),
                target: target_region.clone(),
            }));
        }
        placement.group = target_group;
        Ok(())
    }

    pub fn put_object(
        &self,
        repo: &RepoId,
        kind: GitObjectKind,
        content: &[u8],
    ) -> Result<ContentHash, GitPackError> {
        self.require_placed(repo)?;
        let address = git_object_address(kind, content);
        let framed = frame_git_object(kind, content);
        let native = self.blobs.put(&self.tenant, &framed)?;
        self.link_sha_to_native(repo, &address, &native)?;
        Ok(address)
    }

    pub fn get_object(
        &self,
        repo: &RepoId,
        address: &ContentHash,
    ) -> Result<Vec<u8>, GitPackError> {
        self.get_object_bounded(repo, address, GIT_PACK_OBJECT_MAX_STORED_BYTES)
    }

    pub fn get_object_bounded(
        &self,
        repo: &RepoId,
        address: &ContentHash,
        maximum_stored_bytes: usize,
    ) -> Result<Vec<u8>, GitPackError> {
        let (native, stored_len) = self.object_native_address_and_stored_len(repo, address)?;
        if stored_len > maximum_stored_bytes {
            return Err(GitPackError::ReadLimitExceeded {
                actual: stored_len,
                maximum: maximum_stored_bytes,
            });
        }
        let framed = self.blobs.get(&self.tenant, &native)?;
        let actual_sha = ContentHash::sha256(&framed);
        if &actual_sha != address {
            return Err(GitPackError::Blob(BlobError::IntegrityFail {
                requested: address.clone(),
                actual: actual_sha,
            }));
        }
        Ok(unframe_git_object(&framed))
    }

    pub fn object_stored_len(
        &self,
        repo: &RepoId,
        address: &ContentHash,
    ) -> Result<usize, GitPackError> {
        self.object_native_address_and_stored_len(repo, address)
            .map(|(_, stored_len)| stored_len)
    }

    pub fn get_object_with_recovery<R: BlobStore>(
        &self,
        repo: &RepoId,
        git_sha: &ContentHash,
        replica: &GitPackTier<R>,
    ) -> Result<Vec<u8>, GitPackError> {
        match self.get_object(repo, git_sha) {
            Ok(bytes) => Ok(bytes),
            Err(GitPackError::Blob(BlobError::IntegrityFail { .. })) => {
                replica.get_object(repo, git_sha)
            }
            Err(other) => Err(other),
        }
    }

    pub fn put_pack(
        &self,
        repo: &RepoId,
        packfile_bytes: &[u8],
        objects: Vec<(GitObjectKind, ContentHash)>,
    ) -> Result<PackManifest, GitPackError> {
        self.require_placed(repo)?;
        let pack_hash = self.blobs.put(&self.tenant, packfile_bytes)?;
        Ok(PackManifest { pack_hash, objects })
    }

    pub fn get_pack(
        &self,
        repo: &RepoId,
        pack_hash: &ContentHash,
    ) -> Result<Vec<u8>, GitPackError> {
        self.require_placed(repo)?;
        let metadata = self.blobs.head(&self.tenant, pack_hash)?;
        if metadata.stored_len > GIT_PACKFILE_MAX_STORED_BYTES {
            return Err(GitPackError::ReadLimitExceeded {
                actual: metadata.stored_len,
                maximum: GIT_PACKFILE_MAX_STORED_BYTES,
            });
        }
        Ok(self.blobs.get(&self.tenant, pack_hash)?)
    }

    fn require_placed(&self, repo: &RepoId) -> Result<(), GitPackError> {
        if self.state()?.placements.contains_key(repo) {
            Ok(())
        } else {
            Err(GitPackError::RepoNotPlaced { repo: repo.clone() })
        }
    }

    fn link_sha_to_native(
        &self,
        repo: &RepoId,
        sha: &ContentHash,
        native: &ContentHash,
    ) -> Result<(), GitPackError> {
        self.state()?
            .sha_index
            .insert((repo.clone(), sha.clone()), native.clone());
        Ok(())
    }

    fn native_for_sha(
        &self,
        repo: &RepoId,
        sha: &ContentHash,
    ) -> Result<Option<ContentHash>, GitPackError> {
        Ok(self
            .state()?
            .sha_index
            .get(&(repo.clone(), sha.clone()))
            .cloned())
    }

    fn object_native_address_and_stored_len(
        &self,
        repo: &RepoId,
        address: &ContentHash,
    ) -> Result<(ContentHash, usize), GitPackError> {
        self.require_placed(repo)?;
        let native = self.native_for_sha(repo, address)?.ok_or_else(|| {
            GitPackError::Blob(BlobError::NotFound {
                tenant: self.tenant.clone(),
                hash: address.clone(),
            })
        })?;
        let stored_len = self.blobs.head(&self.tenant, &native)?.stored_len;
        Ok((native, stored_len))
    }

    #[doc(hidden)]
    pub fn native_addr_for_test(
        &self,
        repo: &RepoId,
        sha: &ContentHash,
    ) -> Result<Option<ContentHash>, GitPackError> {
        self.native_for_sha(repo, sha)
    }

    fn state(&self) -> Result<MutexGuard<'_, GitPackState>, GitPackError> {
        self.state
            .lock()
            .map_err(|_| GitPackError::StateUnavailable {
                state: "placement and object index",
            })
    }
}

fn frame_git_object(kind: GitObjectKind, content: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(content.len() + 32);
    framed.extend_from_slice(kind.header_keyword().as_bytes());
    framed.push(b' ');
    framed.extend_from_slice(content.len().to_string().as_bytes());
    framed.push(0);
    framed.extend_from_slice(content);
    framed
}

fn unframe_git_object(framed: &[u8]) -> Vec<u8> {
    match framed.iter().position(|&b| b == 0) {
        Some(nul) => framed[nul + 1..].to_vec(),
        None => framed.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::FsBlobStore;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn placed_tier() -> (GitPackTier<FsBlobStore>, RepoId) {
        let tier = GitPackTier::new(tenant(), FsBlobStore::new());
        let repo = RepoId::from_token("web");
        tier.place_repo(
            repo.clone(),
            RepoGitPlacement {
                group: StorageGroup::from_token("pack-0"),
                region: Region::new("eu-west"),
                status: RepoPlacementStatus::Active,
            },
        )
        .expect("place test repository");
        (tier, repo)
    }

    #[test]
    fn git_object_is_addressed_through_the_trait_and_round_trips() {
        let (tier, repo) = placed_tier();
        let content = b"fn main() {}\n";
        let address = tier
            .put_object(&repo, GitObjectKind::Blob, content)
            .expect("put");

        assert_eq!(address.algorithm(), crate::blob::HashAlgo::Sha256);
        assert_eq!(address, git_object_address(GitObjectKind::Blob, content));
        assert!(address.to_multihash_string().starts_with("sha256:"));

        let got = tier.get_object(&repo, &address).expect("get");
        assert_eq!(
            got, content,
            "the exact object content round-trips through the trait"
        );
    }

    #[test]
    fn bounded_object_read_checks_metadata_before_materialization() {
        let (tier, repo) = placed_tier();
        let content = b"bounded object";
        let address = tier
            .put_object(&repo, GitObjectKind::Blob, content)
            .expect("put");
        let stored_bytes = frame_git_object(GitObjectKind::Blob, content).len();

        assert_eq!(
            tier.get_object_bounded(&repo, &address, stored_bytes)
                .expect("exact limit accepted"),
            content
        );
        assert_eq!(
            tier.get_object_bounded(&repo, &address, stored_bytes - 1),
            Err(GitPackError::ReadLimitExceeded {
                actual: stored_bytes,
                maximum: stored_bytes - 1,
            })
        );
    }

    #[test]
    fn placement_of_returns_region_pinned_relocatable_placement() {
        let (tier, repo) = placed_tier();
        let p = tier
            .placement_of(&repo)
            .expect("placement state")
            .expect("placed");
        assert_eq!(p.group.as_str(), "pack-0");
        assert_eq!(p.region.as_str(), "eu-west");
        assert_eq!(p.status, RepoPlacementStatus::Active);
        assert!(tier
            .placement_of(&RepoId::from_token("ghost"))
            .expect("placement state")
            .is_none());
    }

    #[test]
    fn relocation_does_not_recompute_an_address() {
        let (tier, repo) = placed_tier();
        let content = b"tree content";
        let before = tier
            .put_object(&repo, GitObjectKind::Tree, content)
            .expect("put");

        tier.relocate(
            &repo,
            StorageGroup::from_token("pack-7"),
            &Region::new("eu-west"),
        )
        .expect("a same-region relocation is admitted");

        let p = tier.placement_of(&repo).unwrap().unwrap();
        assert_eq!(p.group.as_str(), "pack-7", "only the stored group flipped");
        assert_eq!(
            p.region.as_str(),
            "eu-west",
            "region unchanged (same-region move)"
        );

        let after_addr = git_object_address(GitObjectKind::Tree, content);
        assert_eq!(
            before, after_addr,
            "the object's content address is unchanged by relocation"
        );
        assert_eq!(
            tier.get_object(&repo, &before)
                .expect("served after relocation"),
            content,
            "the object is still served by the SAME address after relocation (never node-pinned)"
        );
    }

    #[test]
    fn cross_region_relocation_is_refused() {
        let (tier, repo) = placed_tier();
        let e = tier
            .relocate(
                &repo,
                StorageGroup::from_token("pack-n"),
                &Region::new("eu-north"),
            )
            .expect_err("a cross-region relocation target is refused (the residency pin)");
        assert!(
            matches!(
                e,
                GitPackError::Placement(PlacementError::CrossRegion { .. })
            ),
            "{e}"
        );
        let p = tier.placement_of(&repo).unwrap().unwrap();
        assert_eq!(
            p.group.as_str(),
            "pack-0",
            "the rejected relocation did not move the repo"
        );
        assert_eq!(p.region.as_str(), "eu-west");
    }

    #[test]
    fn corrupt_object_is_detected_and_refused_zero_silent_serve() {
        let (tier, repo) = placed_tier();
        let content = b"commit content to corrupt";
        let address = tier
            .put_object(&repo, GitObjectKind::Commit, content)
            .expect("put");

        assert_eq!(tier.get_object(&repo, &address).expect("clean"), content);
        assert_eq!(tier.blobs().telemetry().blob_integrity_fail(), 0);

        let native = tier
            .native_for_sha(&repo, &address)
            .expect("object index state")
            .expect("linked");
        assert!(
            tier.blobs().corrupt_for_drill(&tenant(), &native),
            "object present to corrupt"
        );

        match tier.get_object(&repo, &address) {
            Err(GitPackError::Blob(BlobError::IntegrityFail { .. })) => {}
            Ok(bytes) => panic!("SILENT WRONG-BYTES SERVE - STOR-D7 on packs breached: {bytes:?}"),
            Err(other) => panic!("expected IntegrityFail, got {other}"),
        }
        assert_eq!(
            tier.blobs().telemetry().blob_integrity_fail(),
            1,
            "a corrupt git object read must increment blob_integrity_fail (0 silent serve)"
        );
    }

    #[test]
    fn corrupt_primary_recovers_from_replica() {
        let (primary, repo) = placed_tier();
        let replica = GitPackTier::new(tenant(), FsBlobStore::new());
        replica
            .place_repo(
                repo.clone(),
                RepoGitPlacement {
                    group: StorageGroup::from_token("pack-0"),
                    region: Region::new("eu-west"),
                    status: RepoPlacementStatus::Active,
                },
            )
            .expect("place replica repository");

        let content = b"the authoritative object bytes";
        let address = primary
            .put_object(&repo, GitObjectKind::Blob, content)
            .expect("primary put");
        let replica_addr = replica
            .put_object(&repo, GitObjectKind::Blob, content)
            .expect("replica put");
        assert_eq!(
            address, replica_addr,
            "the same content has the same address on both backings"
        );

        let native = primary.native_for_sha(&repo, &address).unwrap().unwrap();
        assert!(primary.blobs().corrupt_for_drill(&tenant(), &native));

        let recovered = primary
            .get_object_with_recovery(&repo, &address, &replica)
            .expect("recovered from the replica by content address");
        assert_eq!(
            recovered, content,
            "the good replica copy recovers the corrupt object"
        );
        assert_eq!(
            primary.blobs().telemetry().blob_integrity_fail(),
            1,
            "the corrupt primary was detected"
        );
    }

    #[test]
    fn pack_op_on_unplaced_repo_is_refused() {
        let tier = GitPackTier::new(tenant(), FsBlobStore::new());
        let ghost = RepoId::from_token("ghost");
        assert!(matches!(
            tier.put_object(&ghost, GitObjectKind::Blob, b"x"),
            Err(GitPackError::RepoNotPlaced { .. })
        ));
        let addr = git_object_address(GitObjectKind::Blob, b"x");
        assert!(matches!(
            tier.get_object(&ghost, &addr),
            Err(GitPackError::RepoNotPlaced { .. })
        ));
    }

    #[test]
    fn unavailable_pack_state_is_not_reported_as_an_unplaced_repository() {
        let (tier, repo) = placed_tier();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = tier.state.lock().unwrap();
            panic!("poison the test pack state");
        }));

        assert_eq!(
            tier.placement_of(&repo).unwrap_err(),
            GitPackError::StateUnavailable {
                state: "placement and object index",
            }
        );
        assert_eq!(
            tier.put_object(&repo, GitObjectKind::Blob, b"never written")
                .unwrap_err(),
            GitPackError::StateUnavailable {
                state: "placement and object index",
            }
        );
    }

    #[test]
    fn packfile_is_content_addressed_and_corrupt_pack_is_refused() {
        let (tier, repo) = placed_tier();
        let obj_addr = git_object_address(GitObjectKind::Blob, b"member");
        let packfile = b"PACK\0\0\0\x02...opaque packfile bytes...";
        let manifest = tier
            .put_pack(
                &repo,
                packfile,
                vec![(GitObjectKind::Blob, obj_addr.clone())],
            )
            .expect("put pack");
        assert_eq!(manifest.objects, vec![(GitObjectKind::Blob, obj_addr)]);

        assert_eq!(
            tier.get_pack(&repo, &manifest.pack_hash).expect("get pack"),
            packfile
        );

        assert!(tier
            .blobs()
            .corrupt_for_drill(&tenant(), &manifest.pack_hash));
        assert!(matches!(
            tier.get_pack(&repo, &manifest.pack_hash),
            Err(GitPackError::Blob(BlobError::IntegrityFail { .. }))
        ));
    }

    #[test]
    fn git_object_framing_round_trips() {
        let content = b"hello";
        let framed = frame_git_object(GitObjectKind::Blob, content);
        assert_eq!(&framed[..7], b"blob 5\0");
        assert_eq!(unframe_git_object(&framed), content);
        assert_eq!(
            git_object_address(GitObjectKind::Blob, content),
            ContentHash::sha256(&framed)
        );
        assert_eq!(GitObjectKind::Commit.header_keyword(), "commit");
        assert_eq!(GitObjectKind::Tree.header_keyword(), "tree");
        assert_eq!(GitObjectKind::Tag.header_keyword(), "tag");
    }

    #[test]
    fn errors_display_loud_and_specific() {
        let cross = GitPackError::Placement(PlacementError::CrossRegion {
            repo: RepoId::from_token("web"),
            pinned: Region::new("eu-west"),
            target: Region::new("eu-north"),
        });
        let s = cross.to_string();
        assert!(s.contains("eu-west") && s.contains("eu-north"), "{s}");
        assert!(s.contains("never leave their region"), "{s}");

        let not_placed = GitPackError::RepoNotPlaced {
            repo: RepoId::from_token("ghost"),
        };
        assert!(not_placed.to_string().contains("not placed"));
        assert!(not_placed.to_string().contains("ghost"), "{not_placed}");
        assert_eq!(RepoId::from_token("ghost").as_str(), "ghost");
    }
}

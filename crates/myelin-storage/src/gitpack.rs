//! # Local-disk git pack/object storage behind the [`BlobStore`] trait (P-ST-22 → global P-252)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §3.5 (the git object-backing
//! seam STOR-5 — **git packs + loose objects are addressed THROUGH the `BlobStore` trait so the
//! "local-disk → object-store-backed packs" transition is a backing SWAP, not a rewrite**; the
//! v1 git data model is **never node-pinned**; **relocatable placement, repo-granular,
//! region-pinned NOT node-pinned**; the object-backed pack/delta impl is the M5/Git deliverable).
//! Contract-index row 11.2 (the local-disk git pack tier behind the trait — the object-backed
//! pack/delta seam is P-ST-31; the C3 CDN class is P-ST-23) + row 12.2 (repo-granular
//! relocatable `placement_of`).
//!
//! ## What this prompt (P-ST-22 / P-252) ships — and what it deliberately REUSES (EI-01 §7)
//! The content-addressed [`BlobStore`] trait, the BLAKE3/SHA-256 self-describing multihash
//! ([`ContentHash`]), the per-tenant keyspace, and the re-hash-on-read integrity refusal already
//! exist in [`crate::blob`] (shipped by P-ST-03). This module does **NOT** re-define them — the
//! git pack tier is built **on top of** that trait so it is the very seam §3.5 mandates. What is
//! genuinely NEW here:
//!
//! 1. **Git objects addressed THROUGH the trait** ([`GitPackTier::put_object`] /
//!    [`GitPackTier::put_pack`]) — a git loose object (or a packfile) is content-addressed and
//!    stored via a [`BlobStore`], NOT a raw filesystem path. Git addresses its objects by **SHA**
//!    (SHA-1 legacy / SHA-256 modern); the tier writes them under the `sha256:` multihash tag the
//!    BlobStore admits, so a git object coexists with native BLAKE3 blobs and the read-side
//!    SHA-256 verify (P-ST-22 closed in [`crate::blob`]) detects a corrupt object. **The address
//!    is the bytes' hash, not a node path** — the property an object-store backing relocates by.
//! 2. **Repo-granular, region-pinned, NEVER node-pinned placement** ([`RepoGitPlacement`] +
//!    [`GitPackTier::placement_of`] / [`GitPackTier::relocate`]) — the storage-grain realisation
//!    of contract 12.2: every repo carries a placement `{group, region, status}` that is a
//!    **stored fact**, region-pinned (a repo never leaves its region) and **relocatable within
//!    its region without recomputing any address** (relocation flips the stored group only; the
//!    objects' content addresses are byte-identical before + after — they are NOT keyed by a node
//!    hash). This is exactly the relocatability §3.5 **DECIDES now** so local-disk → object-store
//!    is a backing swap.
//! 3. **STOR-D7 on git packs** ([`GitPackTier::get_object`] / [`GitPackTier::get_pack`]) — a
//!    corrupt pack object is detected by the BlobStore's re-hash-on-read (content-address
//!    mismatch) and **refused** (0 silent serve); `blob_integrity_fail` increments. Recovery is
//!    by re-fetching the same content address from a replica/backup tier (a SECOND [`BlobStore`]),
//!    modelled by [`GitPackTier::get_object_with_recovery`].
//!
//! ## A repo's residency is its region (the load-bearing pin — §3.5 / contract 12.2)
//! [`RepoGitPlacement::region`] is the repo's region of record. The tier REFUSES a relocation
//! whose target region differs from the repo's pinned region ([`PlacementError::CrossRegion`]) —
//! the residency pin holds at repo grain: a git repo's packs never leave its region. This module
//! does NOT author the tenant↔region authority (that is the control plane's
//! `placement_of`/`residency_verify`, 12.2/12.4, consumed in `myelin-control-plane`); it models
//! the STORAGE face — a region-pinned, node-relocatable pack tier — which the control-plane
//! routing answer ([`super`] note) sits above.
//!
//! **DEVIATION (EI-01 §1, documented):** contract 12.2's `placement_of(repo)` is OWNED by
//! `myelin-control-plane` (P-CP-15 / P-250). `myelin-storage` sits BELOW the control plane in the
//! §2.9 DAG and must NOT depend on it, so this module does not import `RepoPlacement` — it carries
//! the *storage-grain* placement value the pack tier needs (`{group, region, status}`, the same
//! shape minus the routing-only `cell_id`, which is a control-plane concern). The two are
//! reconciled by shape, not by a shared type: the control plane routes a repo to a cell+group; the
//! storage tier pins+relocates the repo's packs within a region by group. Recorded as a faithful
//! split, not a duplicate type.
//!
//! ## Floor named (designed-not-built → filling prompt) — VISION §3 / EI-01 §1
//! - **Local-disk packs behind the trait is THE FLOOR.** The **object-backed pack/delta
//!   management + smart-transport** is the **M5 follow-on P-ST-31 (global P-442)** — a backing
//!   swap by the trait's design (trigger: the single-node ceiling measured, GIT-D4). The
//!   relocatability constraint is DECIDED here; the object-backed pack IMPL (chunking, delta-base
//!   selection, serving from the object tier) is co-owned with the Git subsystem at M5. The fs
//!   floor here uses [`FsBlobStore`]; swapping it for the object-store `S3BlobStore` (P-ST-30) is a
//!   one-line backing change because everything is addressed through the trait. Recorded HERE in
//!   writing.
//! - **The within-EU CDN clone/bundle blob class (C3) is the SIBLING prompt P-ST-23 (global
//!   P-254)** — clone bundles are content-addressed T2 blobs riding THIS pack tier; not built
//!   here. Recorded HERE in writing.
//! - **Real git pack PARSING (delta resolution, the v2/v3 packfile index)** is the Git subsystem's
//!   job; this tier treats a packfile as an **opaque content-addressed blob** plus a manifest of
//!   the SHA-addressed objects it contains. The integrity property (re-hash-on-read refuses a
//!   corrupt object) does not need the parse — it needs the content address, which it has.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The **trait-addressing of packs (relocatability)** is mandatory-core: a git repo pinned to a
//! node would break the object-store swap §3.5 forbids. The load-bearing mutants — the
//! relocation cross-region reject ([`GitPackTier::relocate`]'s `target.region != region`
//! branch), the relocation-does-not-recompute-an-address property, the put-through-the-trait
//! SHA-256 tag, and the corrupt-object refusal (inherited from [`crate::blob`]'s re-hash-on-read,
//! re-asserted on packs) — are each killed by an assertion in the unit + drill tests. The floor
//! is **≥ 80%**.

use std::collections::HashMap;
use std::sync::Mutex;

use myelin_tenancy::{Region, TenantId};

use crate::blob::{BlobError, BlobStore, ContentHash};

/// **A git object kind (git's four object types).** PII-free — a closed enum tag. The tier stores
/// loose objects of any kind; the kind is carried in the object's git header (`<kind> <len>\0…`)
/// which is part of the SHA-addressed bytes, so it does not need a separate field on the address.
/// It is surfaced here for the pack manifest (which objects a packfile contains).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GitObjectKind {
    /// A commit object.
    Commit,
    /// A tree object (a directory listing).
    Tree,
    /// A blob object (file content).
    Blob,
    /// An annotated tag object.
    Tag,
}

impl GitObjectKind {
    /// The git header keyword for this kind (`commit`/`tree`/`blob`/`tag`) — the prefix of the
    /// `<kind> <len>\0<content>` framing git hashes. Used to build the SHA-addressed object bytes.
    pub fn header_keyword(self) -> &'static str {
        match self {
            GitObjectKind::Commit => "commit",
            GitObjectKind::Tree => "tree",
            GitObjectKind::Blob => "blob",
            GitObjectKind::Tag => "tag",
        }
    }
}

/// **Build the SHA-256 content address of a git object from its kind + content.** Git addresses an
/// object by `SHA(<kind> <len>\0<content>)` (the loose-object framing). This produces the
/// `sha256:`-tagged [`ContentHash`] the pack tier stores the object under — so the BlobStore's
/// re-hash-on-read verifies it under SHA-256 (P-ST-22). Modern git uses SHA-256 (the
/// `extensions.objectFormat = sha256` repos); the legacy SHA-1 object format is a separate
/// algorithm tag a later prompt may admit (not on this floor — never a silent mis-verify).
pub fn git_object_address(kind: GitObjectKind, content: &[u8]) -> ContentHash {
    ContentHash::sha256(&frame_git_object(kind, content))
}

/// **The placement lifecycle status of a repo's git pack tier (storage grain).** Mirrors the
/// control-plane placement status by shape (a repo on an offboarding tenant is offboarding) but
/// is the STORAGE face — PII-free closed enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoPlacementStatus {
    /// The repo is actively placed and serving.
    Active,
    /// The repo's tenant is offboarding — the packs are pending crypto-shred (the erasure reach
    /// into pack backups is P-ST-24).
    Offboarding,
}

/// **A repo's git-pack placement (the storage face of contract 12.2 — region-pinned, relocatable,
/// NEVER node-pinned).** A **stored fact**: the repo's storage `group` within its region, the
/// repo's pinned `region`, and the placement `status`. There is **no node id / node hash** on this
/// value by construction — a repo's packs are addressed by content (their SHA), so the placement
/// can be relocated within the region (the `group` flips) without moving or re-addressing a single
/// object. This is the relocatability §3.5 decides now so local-disk → object-store is a backing
/// swap, not a rewrite.
///
/// See the module-level DEVIATION note on why this is a storage-grain value and not the
/// control-plane `RepoPlacement` type (the DAG forbids the dependency; the routing-only `cell_id`
/// is a control-plane concern).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoGitPlacement {
    /// The storage group the repo's pack tier lives in WITHIN its region (Storage 11.2 pack tier).
    /// A **stored fact**, relocatable within-region — NOT a node hash. Opaque, PII-free.
    pub group: StorageGroup,
    /// The repo's pinned residency region. A repo NEVER leaves this region; relocation is a
    /// same-region move by construction (the tier rejects a cross-region target).
    pub region: Region,
    /// The placement lifecycle status. PII-free closed enum.
    pub status: RepoPlacementStatus,
}

/// **A repo-storage group within a region (architecture §5.2 / 11.2 pack tier).** PII-free — an
/// opaque storage-group label (where the repo's packs live within its region). A repo's `group` is
/// a stored fact alongside its region; relocation flips the group without changing any object's
/// content address.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StorageGroup(String);

impl StorageGroup {
    /// Construct a `StorageGroup` from an opaque storage-group token (never personal data).
    #[inline]
    pub fn from_token(token: impl Into<String>) -> StorageGroup {
        StorageGroup(token.into())
    }

    /// The opaque storage-group token (a placement label — no PII inside).
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An opaque repo identity (the repo's stable id — its clone URL `tenant/repo` is NOT a node pin).
/// PII-free: a tenant-scoped repo name, never personal data.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoId(String);

impl RepoId {
    /// Construct a `RepoId` from an opaque token (e.g. the repo slug). PII-free.
    #[inline]
    pub fn from_token(token: impl Into<String>) -> RepoId {
        RepoId(token.into())
    }

    /// The opaque repo token as a string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// **An error from the git pack tier.** Distinct from [`BlobError`] (the underlying trait error):
/// these are pack-tier / placement errors. A read-integrity failure surfaces as the inner
/// [`BlobError::IntegrityFail`] (the STOR-D7 refusal) wrapped in [`GitPackError::Blob`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitPackError {
    /// A pack-tier operation referenced a repo that is not registered with the tier (fail-closed —
    /// never fabricate a placement).
    RepoNotPlaced {
        /// The repo that has no placement.
        repo: RepoId,
    },
    /// The underlying [`BlobStore`] failed — including the **STOR-D7 integrity refusal** (a
    /// corrupt pack object was detected on read and refused, 0 silent serve).
    Blob(BlobError),
    /// A placement / relocation was rejected by the residency pin or an invariant.
    Placement(PlacementError),
    /// A read was refused because a byte or item ceiling would be exceeded.
    ReadLimitExceeded {
        /// The observed size or count.
        actual: usize,
        /// The caller's maximum allowance in the same unit.
        maximum: usize,
    },
}

/// **The reason a repo placement / relocation is rejected.**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlacementError {
    /// A relocation target region differs from the repo's pinned region — **the residency pin at
    /// repo grain**: a repo's packs never leave their region (§3.5 / contract 12.2). 0 repos cross
    /// the region boundary.
    CrossRegion {
        /// The repo being relocated.
        repo: RepoId,
        /// The repo's pinned region (of record).
        pinned: Region,
        /// The rejected target region.
        target: Region,
    },
}

impl std::fmt::Display for GitPackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitPackError::RepoNotPlaced { repo } => write!(
                f,
                "git pack tier: repo `{}` is not placed (fail-closed — no pack tier for an \
                 unregistered repo)",
                repo.as_str()
            ),
            GitPackError::Blob(e) => write!(f, "git pack tier blob error: {e}"),
            GitPackError::Placement(e) => write!(f, "git pack placement rejected: {e}"),
            GitPackError::ReadLimitExceeded {
                actual,
                maximum,
            } => write!(
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
                "repo `{}` is pinned to region `{}` — a relocation to `{}` is REFUSED (a repo's \
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

/// Backstop for direct object reads whose caller does not impose a tighter transport limit.
pub const GIT_PACK_OBJECT_MAX_STORED_BYTES: usize = 512 * 1024 * 1024;
/// Backstop for direct opaque pack reads whose caller does not impose a tighter transport limit.
pub const GIT_PACKFILE_MAX_STORED_BYTES: usize = 1024 * 1024 * 1024;

/// One stored packfile's manifest entry — the opaque packfile blob's content address plus the
/// SHA-addressed objects it contains (so a reader can resolve an object to its pack). PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackManifest {
    /// The content address of the opaque packfile blob (addressed through the trait).
    pub pack_hash: ContentHash,
    /// The SHA-addressed objects this packfile contains (kind + address). The tier treats the
    /// packfile as opaque; the manifest is the index the Git subsystem's real pack parser builds.
    pub objects: Vec<(GitObjectKind, ContentHash)>,
}

/// **The local-disk git pack/object tier behind the [`BlobStore`] trait (contract 11.2, P-ST-22).**
///
/// Git packs + loose objects are addressed through a [`BlobStore`] (the fs floor [`FsBlobStore`]
/// on this band; the object-store backing is the one-line P-ST-30/P-ST-31 swap). Every repo
/// carries a region-pinned, node-relocatable [`RepoGitPlacement`] (the storage face of 12.2). The
/// tier is generic over the blob store `B` so the object-store swap is a type parameter, never a
/// rewrite — that genericity IS the §3.5 seam.
///
/// `tenant` scopes the BlobStore keyspace (per-tenant isolation — the §3.2 rule); the placement
/// pins the repo's region within that tenant.
pub struct GitPackTier<B: BlobStore> {
    /// The tenant whose keyspace the packs live in (per-tenant blob isolation, §3.2).
    tenant: TenantId,
    /// The content-addressed object/pack backing — the SEAM (fs floor → object store swap).
    blobs: B,
    /// Per-repo stored placement facts (region-pinned, relocatable). Keyed by the opaque repo id.
    placements: Mutex<HashMap<RepoId, RepoGitPlacement>>,
    /// The git-SHA → native-blob-address index, keyed by `(repo, git-SHA)`. A git object's identity
    /// is its SHA; the [`BlobStore`] hashes natively (BLAKE3). This index links the two so a get by
    /// git SHA resolves to the stored blob — it is a CONTENT-level index (a SHA→hash map), NOT a
    /// node path, so relocating a repo never touches it (it is relocation-stable).
    sha_index: Mutex<HashMap<(RepoId, ContentHash), ContentHash>>,
}

impl<B: BlobStore> GitPackTier<B> {
    /// Open a git pack tier over a content-addressed blob backing for `tenant`. No repos are placed
    /// yet — [`Self::place_repo`] registers each repo's region-pinned placement.
    pub fn new(tenant: TenantId, blobs: B) -> GitPackTier<B> {
        GitPackTier {
            tenant,
            blobs,
            placements: Mutex::new(HashMap::new()),
            sha_index: Mutex::new(HashMap::new()),
        }
    }

    /// The tenant whose keyspace this tier's packs live in.
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// The underlying content-addressed blob store (the integrity telemetry lives on it).
    pub fn blobs(&self) -> &B {
        &self.blobs
    }

    /// **Place a repo (register its region-pinned, relocatable placement — a stored fact).** The
    /// repo's `region` is its region of record; relocation can move it within that region but
    /// never out of it. The placement carries NO node id — the repo's packs are content-addressed,
    /// so relocation is a stored-fact flip, not an object move.
    pub fn place_repo(&self, repo: RepoId, placement: RepoGitPlacement) {
        self.placements
            .lock()
            .expect("placement mutex")
            .insert(repo, placement);
    }

    /// **`placement_of(repo) → RepoGitPlacement` (the storage face of contract 12.2).** Returns the
    /// repo's region-pinned, relocatable placement, or `None` for an unregistered repo (never a
    /// fabricated answer — fail-closed). It is a placement answer, never an authz answer.
    pub fn placement_of(&self, repo: &RepoId) -> Option<RepoGitPlacement> {
        self.placements
            .lock()
            .expect("placement mutex")
            .get(repo)
            .cloned()
    }

    /// **Relocate a repo to a DIFFERENT storage group WITHIN its region (relocatable, NEVER
    /// node-pinned).** Flips ONLY the stored `group` — a stored-fact update, **NOT** an object
    /// move or address recompute: every object's content address is byte-identical before + after
    /// (the `relocation_does_not_recompute_an_address` test pins this). A relocation whose
    /// `target_region` differs from the repo's pinned region is **REFUSED**
    /// ([`PlacementError::CrossRegion`]) — the residency pin holds at repo grain.
    ///
    /// **FLOOR (M5, P-ST-31):** this flips the storage routing FACT; the durable workflow that
    /// actually re-homes the pack bytes onto the object tier is the object-backed pack follow-on.
    /// On the local-disk floor the objects already live in one content-addressed store, so a
    /// within-region group flip is purely a placement-fact update.
    pub fn relocate(
        &self,
        repo: &RepoId,
        target_group: StorageGroup,
        target_region: &Region,
    ) -> Result<(), GitPackError> {
        let mut placements = self.placements.lock().expect("placement mutex");
        let placement = placements
            .get_mut(repo)
            .ok_or_else(|| GitPackError::RepoNotPlaced { repo: repo.clone() })?;
        // THE RESIDENCY PIN: a cross-region relocation target is refused. A repo's packs never
        // leave their region (§3.5 / 12.2). This is a stored-fact update — no node hash consulted.
        if target_region != &placement.region {
            return Err(GitPackError::Placement(PlacementError::CrossRegion {
                repo: repo.clone(),
                pinned: placement.region.clone(),
                target: target_region.clone(),
            }));
        }
        // Same-region move: flip ONLY the stored group. No object is moved or re-addressed.
        placement.group = target_group;
        Ok(())
    }

    /// **Put a git loose object THROUGH the trait** (content-addressed, NOT a raw fs path). The
    /// object is SHA-addressed by its git framing ([`git_object_address`]) and stored via the
    /// [`BlobStore`] under that `sha256:` address. Returns the content address — the handle the
    /// repo's ref graph points at. The repo must be placed (fail-closed).
    ///
    /// This is the load-bearing property §3.5 needs: the object is addressed by its CONTENT, not a
    /// node path, so the local-disk → object-store backing is a swap behind the trait.
    pub fn put_object(
        &self,
        repo: &RepoId,
        kind: GitObjectKind,
        content: &[u8],
    ) -> Result<ContentHash, GitPackError> {
        self.require_placed(repo)?;
        // The object's git identity is the SHA-256 of its `<kind> <len>\0<content>` framing. Store
        // the FRAMED bytes THROUGH the trait — the BlobStore content-addresses them natively
        // (BLAKE3) and gives re-hash-on-read integrity on that native address for free. We keep the
        // git SHA as the caller-facing handle and link it to the native address so a get by git SHA
        // resolves to the stored blob and is additionally verified under SHA-256 (the git world's
        // own integrity). NOTHING is keyed by a node path — both addresses are content hashes, so
        // the local-disk → object-store backing is a swap behind the trait (§3.5).
        let address = git_object_address(kind, content);
        let framed = frame_git_object(kind, content);
        let native = self.blobs.put(&self.tenant, &framed)?;
        self.link_sha_to_native(repo, &address, &native);
        Ok(address)
    }

    /// **Get a git object by its SHA address THROUGH the trait, with re-hash-on-read integrity.**
    /// Resolves the git SHA address to the stored blob and serves the **unframed** object content,
    /// re-hashing the framed bytes under SHA-256 — a corrupt object is **refused**
    /// ([`BlobError::IntegrityFail`] → [`GitPackError::Blob`], 0 silent serve), `blob_integrity_fail`
    /// increments (STOR-D7 on git packs).
    pub fn get_object(
        &self,
        repo: &RepoId,
        address: &ContentHash,
    ) -> Result<Vec<u8>, GitPackError> {
        self.get_object_bounded(repo, address, GIT_PACK_OBJECT_MAX_STORED_BYTES)
    }

    /// Read a git object only when its stored framing fits `maximum_stored_bytes`. Metadata is
    /// checked before [`BlobStore::get`], so a rejected object is never materialized in memory.
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
        // Serve the framed bytes through the trait. The BlobStore re-hashes on read on the NATIVE
        // (BLAKE3) address and refuses a corrupt blob FIRST (incrementing `blob_integrity_fail`,
        // 0 silent serve — STOR-D7 on packs). We THEN re-verify under the git SHA so the git-world
        // integrity is independently asserted (defence in depth — a corrupt object is refused under
        // SHA-256 too, never a silent wrong-object serve).
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

    /// Return an object's stored framed length without materializing it.
    pub fn object_stored_len(
        &self,
        repo: &RepoId,
        address: &ContentHash,
    ) -> Result<usize, GitPackError> {
        self.object_native_address_and_stored_len(repo, address)
            .map(|(_, stored_len)| stored_len)
    }

    /// **Get a git object with replica/backup RECOVERY (STOR-D7 "recover from replica/backup").**
    /// If the primary read is refused as corrupt ([`BlobError::IntegrityFail`]), re-fetch the SAME
    /// content address from a replica [`BlobStore`] (a second backing of the same content-addressed
    /// objects). Because the address is the content hash, the replica's copy verifies on read — the
    /// corrupt primary is never silently served, and the good replica copy recovers the object.
    pub fn get_object_with_recovery<R: BlobStore>(
        &self,
        repo: &RepoId,
        git_sha: &ContentHash,
        replica: &GitPackTier<R>,
    ) -> Result<Vec<u8>, GitPackError> {
        match self.get_object(repo, git_sha) {
            Ok(bytes) => Ok(bytes),
            Err(GitPackError::Blob(BlobError::IntegrityFail { .. })) => {
                // The primary is corrupt + refused (0 silent serve). Recover the SAME content
                // address from the replica — content-addressing makes "the same object" verifiable.
                replica.get_object(repo, git_sha)
            }
            Err(other) => Err(other),
        }
    }

    /// **Put a packfile THROUGH the trait** (the opaque content-addressed packfile blob + its
    /// object manifest). The packfile is stored as an opaque blob (the real pack parse is the Git
    /// subsystem's job); the manifest records the SHA-addressed objects it contains. Returns the
    /// [`PackManifest`] (the pack's content address + its object index).
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

    /// **Get a packfile blob by its content address (re-hash-on-read integrity).** A corrupt
    /// packfile is detected by the BlobStore's re-hash-on-read and **refused** (0 silent serve).
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

    // ---- internals ----

    /// Fail-closed: a pack operation on an unregistered repo is refused (no fabricated placement).
    fn require_placed(&self, repo: &RepoId) -> Result<(), GitPackError> {
        if self
            .placements
            .lock()
            .expect("placement mutex")
            .contains_key(repo)
        {
            Ok(())
        } else {
            Err(GitPackError::RepoNotPlaced { repo: repo.clone() })
        }
    }

    /// Link a git SHA address to the native (BLAKE3) blob address it was stored under, per repo. A
    /// git object's identity is its SHA; the BlobStore hashes natively. The link is a content-level
    /// index (NOT a node path) — relocating the repo never touches it, so it is relocation-stable.
    fn link_sha_to_native(&self, repo: &RepoId, sha: &ContentHash, native: &ContentHash) {
        self.sha_index
            .lock()
            .expect("sha index mutex")
            .insert((repo.clone(), sha.clone()), native.clone());
    }

    /// Resolve a git SHA address to the native blob address it is stored under.
    fn native_for_sha(&self, repo: &RepoId, sha: &ContentHash) -> Option<ContentHash> {
        self.sha_index
            .lock()
            .expect("sha index mutex")
            .get(&(repo.clone(), sha.clone()))
            .cloned()
    }

    fn object_native_address_and_stored_len(
        &self,
        repo: &RepoId,
        address: &ContentHash,
    ) -> Result<(ContentHash, usize), GitPackError> {
        self.require_placed(repo)?;
        let native = self.native_for_sha(repo, address).ok_or_else(|| {
            GitPackError::Blob(BlobError::NotFound {
                tenant: self.tenant.clone(),
                hash: address.clone(),
            })
        })?;
        let stored_len = self.blobs.head(&self.tenant, &native)?.stored_len;
        Ok((native, stored_len))
    }

    /// Test/drill-only: the native blob address a git SHA object is stored under, so the STOR-D7
    /// drill can corrupt the underlying blob and prove re-hash-on-read refuses it. Not part of the
    /// production surface (a git object is always referenced by its git SHA address).
    #[doc(hidden)]
    pub fn native_addr_for_test(&self, repo: &RepoId, sha: &ContentHash) -> Option<ContentHash> {
        self.native_for_sha(repo, sha)
    }
}

/// Frame a git object as `<kind> <len>\0<content>` (the loose-object framing git hashes + stores).
fn frame_git_object(kind: GitObjectKind, content: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(content.len() + 32);
    framed.extend_from_slice(kind.header_keyword().as_bytes());
    framed.push(b' ');
    framed.extend_from_slice(content.len().to_string().as_bytes());
    framed.push(0);
    framed.extend_from_slice(content);
    framed
}

/// Recover the object content from its `<kind> <len>\0<content>` framing (strip the header).
fn unframe_git_object(framed: &[u8]) -> Vec<u8> {
    match framed.iter().position(|&b| b == 0) {
        Some(nul) => framed[nul + 1..].to_vec(),
        // No NUL framing (defensive — a non-framed blob); return the bytes as-is.
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
        );
        (tier, repo)
    }

    /// **A git object is addressed THROUGH the trait (not a raw fs path) and round-trips the exact
    /// content under its SHA address.** The handle is the git SHA-256 address — the property an
    /// object-store backing relocates by (content-addressed, never node-pinned).
    #[test]
    fn git_object_is_addressed_through_the_trait_and_round_trips() {
        let (tier, repo) = placed_tier();
        let content = b"fn main() {}\n";
        let address = tier
            .put_object(&repo, GitObjectKind::Blob, content)
            .expect("put");

        // The address is the git SHA-256 of the framed object (the git identity), self-describing.
        assert_eq!(address.algo, crate::blob::HashAlgo::Sha256);
        assert_eq!(address, git_object_address(GitObjectKind::Blob, content));
        assert!(address.to_multihash_string().starts_with("sha256:"));

        // get by the git SHA address returns the EXACT object content (re-hash-on-read verified).
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

    /// **`placement_of(repo)` returns the region-pinned, relocatable placement; an unregistered
    /// repo is `None` (fail-closed).** The storage face of contract 12.2.
    #[test]
    fn placement_of_returns_region_pinned_relocatable_placement() {
        let (tier, repo) = placed_tier();
        let p = tier.placement_of(&repo).expect("placed");
        assert_eq!(p.group.as_str(), "pack-0");
        assert_eq!(p.region.as_str(), "eu-west");
        assert_eq!(p.status, RepoPlacementStatus::Active);
        // No node id on the placement by construction (region-pinned, NOT node-pinned).
        assert!(tier.placement_of(&RepoId::from_token("ghost")).is_none());
    }

    /// **THE RELOCATABILITY PROPERTY (§3.5 / 12.2): relocating a repo within its region flips ONLY
    /// the stored group and does NOT recompute any object's content address.** Every object's git
    /// SHA address is byte-identical before + after; a get still serves it. This is what makes the
    /// local-disk → object-store transition a backing swap (the repo is NEVER node-pinned).
    #[test]
    fn relocation_does_not_recompute_an_address() {
        let (tier, repo) = placed_tier();
        let content = b"tree content";
        let before = tier
            .put_object(&repo, GitObjectKind::Tree, content)
            .expect("put");

        // Relocate to a different group, SAME region.
        tier.relocate(
            &repo,
            StorageGroup::from_token("pack-7"),
            &Region::new("eu-west"),
        )
        .expect("a same-region relocation is admitted");

        // The placement flipped the group; the region is unchanged.
        let p = tier.placement_of(&repo).unwrap();
        assert_eq!(p.group.as_str(), "pack-7", "only the stored group flipped");
        assert_eq!(
            p.region.as_str(),
            "eu-west",
            "region unchanged (same-region move)"
        );

        // The object's address is byte-identical AND still resolvable — no re-address, no move.
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

    /// **THE RESIDENCY PIN: relocating a repo to a CROSS-REGION group is REFUSED.** A repo's git
    /// packs never leave their region (§3.5 / 12.2). 0 repos cross the region boundary; the repo
    /// did not move.
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
        // The repo did NOT move — still on pack-0 in eu-west.
        let p = tier.placement_of(&repo).unwrap();
        assert_eq!(
            p.group.as_str(),
            "pack-0",
            "the rejected relocation did not move the repo"
        );
        assert_eq!(p.region.as_str(), "eu-west");
    }

    /// **STOR-D7 on git packs: a corrupt object is DETECTED on read and REFUSED (0 silent serve);
    /// `blob_integrity_fail` increments.** Re-hash-on-read (through the trait) catches the
    /// content-address mismatch — the most load-bearing assertion of the pack tier.
    #[test]
    fn corrupt_object_is_detected_and_refused_zero_silent_serve() {
        let (tier, repo) = placed_tier();
        let content = b"commit content to corrupt";
        let address = tier
            .put_object(&repo, GitObjectKind::Commit, content)
            .expect("put");

        // Clean read serves the object and does not signal.
        assert_eq!(tier.get_object(&repo, &address).expect("clean"), content);
        assert_eq!(tier.blobs().telemetry().blob_integrity_fail(), 0);

        // Corrupt the stored object at its NATIVE blob address (bit-rot / tamper).
        let native = tier.native_for_sha(&repo, &address).expect("linked");
        assert!(
            tier.blobs().corrupt_for_drill(&tenant(), &native),
            "object present to corrupt"
        );

        // Re-hash-on-read REFUSES — 0 silent serve.
        match tier.get_object(&repo, &address) {
            Err(GitPackError::Blob(BlobError::IntegrityFail { .. })) => {}
            Ok(bytes) => panic!("SILENT WRONG-BYTES SERVE — STOR-D7 on packs breached: {bytes:?}"),
            Err(other) => panic!("expected IntegrityFail, got {other}"),
        }
        assert_eq!(
            tier.blobs().telemetry().blob_integrity_fail(),
            1,
            "a corrupt git object read must increment blob_integrity_fail (0 silent serve)"
        );
    }

    /// **STOR-D7 recovery: a corrupt PRIMARY object is refused, then RECOVERED from a replica by
    /// the SAME content address.** Content-addressing makes "the same object" verifiable on the
    /// replica — the corrupt primary is never silently served, the good replica copy recovers it.
    #[test]
    fn corrupt_primary_recovers_from_replica() {
        let (primary, repo) = placed_tier();
        // A replica tier (a second content-addressed backing of the same objects), same placement.
        let replica = GitPackTier::new(tenant(), FsBlobStore::new());
        replica.place_repo(
            repo.clone(),
            RepoGitPlacement {
                group: StorageGroup::from_token("pack-0"),
                region: Region::new("eu-west"),
                status: RepoPlacementStatus::Active,
            },
        );

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

        // Corrupt the PRIMARY's copy.
        let native = primary.native_for_sha(&repo, &address).unwrap();
        assert!(primary.blobs().corrupt_for_drill(&tenant(), &native));

        // Recovery: the primary read is refused (0 silent serve) and the SAME address recovers the
        // good object from the replica.
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

    /// **Fail-closed: a pack operation on an UNREGISTERED repo is refused** (no fabricated
    /// placement — never put/get for a repo the tier does not know).
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

    /// **A packfile is stored as an opaque content-addressed blob + a manifest; a corrupt packfile
    /// is detected on read (0 silent serve).** The real pack parse is the Git subsystem's job; the
    /// integrity property needs only the content address.
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

        // The packfile round-trips by its content address.
        assert_eq!(
            tier.get_pack(&repo, &manifest.pack_hash).expect("get pack"),
            packfile
        );

        // Corrupt the packfile blob → re-hash-on-read refuses it (0 silent serve).
        assert!(tier
            .blobs()
            .corrupt_for_drill(&tenant(), &manifest.pack_hash));
        assert!(matches!(
            tier.get_pack(&repo, &manifest.pack_hash),
            Err(GitPackError::Blob(BlobError::IntegrityFail { .. }))
        ));
    }

    /// The git object framing is `<kind> <len>\0<content>` and round-trips through frame/unframe;
    /// the address matches the SHA-256 of the framing (the git identity).
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
        // The four kinds carry distinct headers.
        assert_eq!(GitObjectKind::Commit.header_keyword(), "commit");
        assert_eq!(GitObjectKind::Tree.header_keyword(), "tree");
        assert_eq!(GitObjectKind::Tag.header_keyword(), "tag");
    }

    /// Errors render loud + specific (a refusal is diagnosable — EI-01 §3).
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
        // The repo id is rendered verbatim in the error (kills the `as_str` mutants — the loud,
        // diagnosable error must name the exact repo, never an empty/constant string).
        assert!(not_placed.to_string().contains("ghost"), "{not_placed}");
        assert_eq!(RepoId::from_token("ghost").as_str(), "ghost");
    }
}

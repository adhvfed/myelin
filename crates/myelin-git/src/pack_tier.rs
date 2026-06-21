//! # The git PACK TIER on the local-NVMe `BlobStore` floor — GIT-P11 / P-272 (M3-G1)
//!
//! The **git side** of the pack/object tier: the accepted-quarantine → object-DB migration, the
//! **commit-graph + reachability bitmaps + multi-pack-index (MIDX)** maintenance artifacts, and the
//! **clone round-trip** — all addressed THROUGH the [`myelin_storage::BlobStore`] trait so the
//! local-disk → object-store transition is a backing **SWAP, not a rewrite** (storage §3.5, STOR-5).
//! Repos are **RELOCATABLE, never node-pinned** (the inherited non-negotiable 8 / STOR-5).
//!
//! ## What is REUSED vs what is NEW here (coherence — EI-01 §7)
//! The byte-addressing seam already exists and is NOT re-defined:
//! - [`myelin_storage::BlobStore`] — the content-addressed put/get trait (P-ST-03).
//! - [`myelin_storage::GitPackTier`] — git objects/packfiles addressed THROUGH the trait + the
//!   region-pinned, **relocatable, never node-pinned** [`myelin_storage::RepoGitPlacement`] (the
//!   storage face of contract 12.2) + the corrupt-object re-hash-on-read refusal (P-252 / P-ST-22).
//!
//! What is **genuinely NEW** at GIT-P11 (the git subsystem owns these — storage treats a packfile
//! as an opaque content-addressed blob):
//! 1. **The git-side object-DB migration** ([`PackObjectDb`]): the [`crate::receive_pack::Quarantine
//!    Migration`] floor `receive_pack.rs` explicitly named GIT-P11 to fill — accepted quarantine
//!    objects are migrated **through the `GitPackTier`** (so they are content-addressed + durable on
//!    the write quorum before the ref CAS), closing that floor with the real pack tier.
//! 2. **The acceleration structures** ([`Maintenance`] → commit-graph / reachability bitmaps /
//!    MIDX): kept fresh on a cadence after ref-update bursts (arch `01 §4.1`, `02 §8`), each written
//!    as a content-addressed blob through the SAME trait, each carrying a **staleness** signal (the
//!    telemetry the maintenance scheduler monitors — contract 1.8). For hot repos these are
//!    mandatory; here the maintenance *result* (the artifact bytes + freshness fence) is the git data
//!    model — the real `git commit-graph write` / `repack -b` / `multi-pack-index` byte production is
//!    the sandboxed wire op (the [`crate::core::Maintenance`] enum routes it to canonical `git`).
//! 3. **The clone round-trip** ([`PackObjectDb::serve_clone`]): a clone served from the local-NVMe
//!    pack tier round-trips **byte-identical** to the receive-pack input (0 corruption; the
//!    commit-graph + bitmaps + MIDX consistent) — the GIT-P11 GATE.
//! 4. **The residency-pin assertion** ([`assert_relocatable_never_node_pinned`]): the git-grain
//!    re-assertion that a repo's pack placement is region-pinned + relocatable and carries **no node
//!    id / node hash** (STOR-5) — the residency-pin lint, green on the pack placement.
//!
//! ## FLOORS NAMED (designed-not-built → filling prompt — VISION §3 / EI-01 §1)
//! - **GF-1 — local-disk packs is THE FLOOR.** Objects + acceleration structures live on the
//!   local-NVMe [`myelin_storage::FsBlobStore`] floor. **Object-backed pack/delta management +
//!   smart-transport read path** is the **M5 follow-on GIT-P33** — a one-line backing swap (the
//!   `B: BlobStore` type parameter), never a rewrite. Trigger: the single-node ceiling measured
//!   (GIT-D4).
//! - **GF-2 — single-cell primary+quorum replication is the FLOOR.** The pack tier here is one
//!   content-addressed backing (+ a replica backing for STOR-D7 recovery); **cross-cell replication**
//!   is the **GIT-P33** follow-on. The object-migration ack models the "durable on the write quorum"
//!   property (arch §4); the real cross-cell quorum is GF-2.
//! - **GF-2b — SHA-1 + `sha1dc` default, hash-AGNOSTIC model is the FLOOR.** Object ids are carried
//!   hash-agnostically ([`crate::receive_pack::Oid`] is rendered hex; the storage address is the git
//!   object's SHA-256 framing, [`myelin_storage::git_object_address`]). The **SHA-256 object-format
//!   flip** (`extensions.objectFormat = sha256`) is the **GIT-P33** follow-on; nothing here pins a
//!   single hash width.
//! - **GF-4 — the large-but-normal monorepo is the FLOOR.** Geometric repack + commit-graph + bitmaps
//!   + MIDX serve a large-but-normal monorepo; the **Mononoke-class segmented backend** is M5+,
//!   triggered by the GIT-D4 ceiling. Named here; not built.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The pack path is mandatory-core. The load-bearing mutants — the migration-through-the-trait
//! (objects are content-addressed, never raw-pathed), the clone round-trip byte-identity, the
//! acceleration-structure staleness fence (a ref-update marks the artifacts stale; maintenance
//! refreshes the fence), and the residency-pin reject (a node-pinned placement is impossible by
//! construction) — are each killed by an assertion in the unit + round-trip tests. The floor is
//! **≥ 80%**.

use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_storage::{
    BlobStore, ContentHash, GitObjectKind, GitPackError, GitPackTier, RepoGitPlacement, RepoId,
};

use crate::receive_pack::{Oid, QuarantineMigration, QuarantineObject};

// ───────────────────────────── the acceleration structures (arch 01 §4.1 / 02 §8) ───────────────

/// **A git acceleration structure** — the three the architecture keeps fresh on a cadence after
/// ref-update bursts (arch `01 §4.1`, `02 §8`). Each is content-addressed (its bytes are a blob
/// through the trait); staleness is a monitored signal (contract 1.8). PII-free closed enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AccelKind {
    /// The **commit-graph** (`commit-graph` file) — O(1) generation-number reachability.
    CommitGraph,
    /// The **reachability bitmaps** (`.bitmap`) — fast `upload-pack` object enumeration.
    Bitmaps,
    /// The **multi-pack-index** (`.midx`) — one index across many packs (avoids loose-pack
    /// pathologies, arch §8).
    Midx,
}

impl AccelKind {
    /// The maintenance op (the [`crate::core::Maintenance`] wire op) that PRODUCES this artifact's
    /// bytes — the sandboxed canonical-`git` invocation the serving tier routes (the byte production
    /// is wire-class; this module owns the artifact's freshness fence + content address).
    pub fn producing_maintenance(self) -> crate::core::Maintenance {
        match self {
            AccelKind::CommitGraph => crate::core::Maintenance::WriteCommitGraph,
            AccelKind::Bitmaps => crate::core::Maintenance::WriteBitmaps,
            AccelKind::Midx => crate::core::Maintenance::WriteMidx,
        }
    }

    /// All three acceleration structures (the maintenance set kept fresh together after a push burst).
    pub fn all() -> [AccelKind; 3] {
        [AccelKind::CommitGraph, AccelKind::Bitmaps, AccelKind::Midx]
    }
}

/// **A maintenance artifact's freshness record.** The artifact bytes are content-addressed through
/// the trait (`blob`); `fresh_at_fence` is the object-DB generation the artifact was built against.
/// When a ref-update advances the object-DB generation past `fresh_at_fence`, the artifact is
/// **stale** ([`PackObjectDb::is_stale`]) — the monitored signal the maintenance scheduler re-runs
/// on (arch §8: "staleness is a monitored signal"). PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccelArtifact {
    /// The content address of the artifact bytes (addressed through the trait — relocation-stable).
    pub blob: ContentHash,
    /// The object-DB generation this artifact was built against (the freshness fence).
    pub fresh_at_fence: u64,
}

// ───────────────────────────── the git-side pack object DB (the GIT-P11 deliverable) ─────────────

/// **The git PACK OBJECT DB on the local-NVMe floor** (contract 11.2 consumed). Wraps the storage
/// [`GitPackTier`] (REUSED, never re-defined) and adds the git-subsystem-owned layer GIT-P11 lands:
/// the accepted-quarantine migration ([`QuarantineMigration`]), the commit-graph/bitmap/MIDX
/// maintenance artifacts + their staleness fences, and the byte-identical clone round-trip.
///
/// Generic over `B: BlobStore` so the local-disk → object-store backing is a **type-parameter swap**
/// (GF-1 → GIT-P33), never a rewrite — that genericity IS the §3.5 seam.
pub struct PackObjectDb<B: BlobStore> {
    /// The storage pack tier — the content-addressed object/packfile backing + the region-pinned,
    /// relocatable placement. REUSED from `myelin-storage` (P-252).
    tier: GitPackTier<B>,
    /// The repo this object DB serves (placed in `tier`).
    repo: RepoId,
    /// The git-SHA (rendered hex, as the wire/ref layer carries it) → storage content-address index,
    /// so a clone can resolve a ref tip's oid back to the stored object. A CONTENT-level index (oid →
    /// hash), NOT a node path — relocating the repo never touches it (relocation-stable).
    oid_index: Mutex<BTreeMap<String, ContentHash>>,
    /// The object-DB generation: bumped on every accepted object migration (a push burst advances
    /// it). The acceleration artifacts' freshness fences compare against it (staleness).
    generation: Mutex<u64>,
    /// The current maintenance artifacts, keyed by kind. Refreshed by [`PackObjectDb::run_maintenance`].
    accel: Mutex<BTreeMap<AccelKind, AccelArtifact>>,
}

impl<B: BlobStore> PackObjectDb<B> {
    /// Open the git object DB over a (already repo-placed) storage pack tier. The repo MUST be placed
    /// in `tier` (region-pinned, relocatable) before objects are migrated — the migration is
    /// fail-closed on an unplaced repo (inherited from the tier).
    pub fn new(tier: GitPackTier<B>, repo: RepoId) -> Self {
        Self {
            tier,
            repo,
            oid_index: Mutex::new(BTreeMap::new()),
            generation: Mutex::new(0),
            accel: Mutex::new(BTreeMap::new()),
        }
    }

    /// The underlying storage pack tier (its placement + integrity telemetry live here).
    pub fn tier(&self) -> &GitPackTier<B> {
        &self.tier
    }

    /// The repo this object DB serves.
    pub fn repo(&self) -> &RepoId {
        &self.repo
    }

    /// **The current placement** (region-pinned, relocatable — the storage face of 12.2). `None`
    /// only if the repo was never placed (fail-closed).
    pub fn placement(&self) -> Option<RepoGitPlacement> {
        self.tier.placement_of(&self.repo)
    }

    /// The current object-DB generation (the freshness fence the acceleration artifacts compare to).
    pub fn generation(&self) -> u64 {
        *self.generation.lock().expect("generation mutex")
    }

    /// **Migrate one accepted quarantine object into the pack tier THROUGH the trait** (the durable
    /// content-addressed write). Returns the storage content address; records the oid → address link
    /// so a clone can resolve the ref tip, and advances the object-DB generation (marking the
    /// acceleration artifacts stale). Fail-closed on an unplaced repo / a backing failure.
    pub fn put_object(
        &self,
        kind: GitObjectKind,
        oid: &Oid,
        content: &[u8],
    ) -> Result<ContentHash, GitPackError> {
        let address = self.tier.put_object(&self.repo, kind, content)?;
        self.oid_index
            .lock()
            .expect("oid index mutex")
            .insert(oid.0.clone(), address.clone());
        // A new object advances the object-DB generation — the acceleration structures built against
        // an earlier generation are now stale (the §8 monitored signal).
        *self.generation.lock().expect("generation mutex") += 1;
        Ok(address)
    }

    /// **Read an object by its git oid** (re-hash-on-read integrity through the trait; a corrupt
    /// object is REFUSED, 0 silent serve — STOR-D7 inherited). The handle is the git oid the ref
    /// graph carries; this resolves it to the stored content address.
    pub fn read_object(&self, oid: &Oid) -> Result<Vec<u8>, GitPackError> {
        let address = self.address_of(oid).ok_or_else(|| GitPackError::RepoNotPlaced {
            repo: self.repo.clone(),
        })?;
        self.tier.get_object(&self.repo, &address)
    }

    /// The storage content address a git oid resolves to (relocation-stable; `None` if not stored).
    pub fn address_of(&self, oid: &Oid) -> Option<ContentHash> {
        self.oid_index.lock().expect("oid index mutex").get(&oid.0).cloned()
    }

    /// **Run a maintenance op: produce + record the acceleration artifact, fresh at the current
    /// generation.** The artifact BYTES are produced by the sandboxed canonical-`git` wire op
    /// ([`AccelKind::producing_maintenance`]) — here the caller passes the produced bytes (the wire
    /// executor's output) and this records them content-addressed through the trait + sets the
    /// freshness fence to the current object-DB generation (so [`Self::is_stale`] reports fresh).
    ///
    /// **FLOOR (GF-1/GF-4):** the real geometric-repack strategy + the byte production run as the
    /// sandboxed wire op; this owns the artifact's content address + freshness fence (the git data
    /// model). On the local-disk floor the artifact is a content-addressed blob like any other.
    pub fn record_maintenance(
        &self,
        kind: AccelKind,
        artifact_bytes: &[u8],
    ) -> Result<AccelArtifact, GitPackError> {
        // The artifact rides the same content-addressed packfile path (an opaque blob through the
        // trait) — so it relocates with the repo and is re-hash-on-read verified.
        let manifest = self.tier.put_pack(&self.repo, artifact_bytes, Vec::new())?;
        let artifact = AccelArtifact {
            blob: manifest.pack_hash,
            fresh_at_fence: self.generation(),
        };
        self.accel
            .lock()
            .expect("accel mutex")
            .insert(kind, artifact.clone());
        Ok(artifact)
    }

    /// Run + record ALL three acceleration structures fresh (the post-push-burst maintenance pass).
    /// `produce` yields the wire op's bytes for each kind (the sandboxed canonical-`git` output).
    pub fn run_maintenance<F>(&self, mut produce: F) -> Result<(), GitPackError>
    where
        F: FnMut(AccelKind) -> Vec<u8>,
    {
        for kind in AccelKind::all() {
            let bytes = produce(kind);
            self.record_maintenance(kind, &bytes)?;
        }
        Ok(())
    }

    /// **Is an acceleration artifact STALE?** `true` iff it does not exist OR its freshness fence is
    /// behind the current object-DB generation (a ref-update burst advanced the generation past it).
    /// The monitored staleness signal (arch §8) the maintenance scheduler re-runs on. A missing
    /// artifact is stale (must be built).
    pub fn is_stale(&self, kind: AccelKind) -> bool {
        match self.accel.lock().expect("accel mutex").get(&kind) {
            None => true,
            Some(a) => a.fresh_at_fence < self.generation(),
        }
    }

    /// The current acceleration artifact for a kind (its content address + freshness fence), if built.
    pub fn accel_artifact(&self, kind: AccelKind) -> Option<AccelArtifact> {
        self.accel.lock().expect("accel mutex").get(&kind).cloned()
    }

    /// **Serve a clone of the named ref tips from the local-NVMe pack tier.** Walks each tip oid's
    /// object out of the tier (re-hash-on-read verified) and returns the `(oid, bytes)` set the
    /// client receives — the bytes a `upload-pack` would stream. The GIT-P11 GATE asserts this
    /// round-trips **byte-identical** to the receive-pack input (0 corruption). A corrupt object is
    /// REFUSED here (never a silent wrong-bytes clone). `tips` are the ref oids to serve.
    pub fn serve_clone(&self, tips: &[Oid]) -> Result<Vec<(Oid, Vec<u8>)>, GitPackError> {
        let mut out = Vec::with_capacity(tips.len());
        for oid in tips {
            let bytes = self.read_object(oid)?;
            out.push((oid.clone(), bytes));
        }
        Ok(out)
    }
}

// ───────────────────────────── the QuarantineMigration impl (closing the named floor) ────────────

/// **The git pack-tier migration sink** — the real [`QuarantineMigration`] the receive-pack path
/// calls on accept (arch §2 step 3). It migrates the accepted quarantine objects **through the
/// `GitPackTier`** (content-addressed, durable on the write quorum) BEFORE the ref CAS — the floor
/// `receive_pack.rs` explicitly named GIT-P11 to fill (replacing the `InMemoryObjectDb` floor with
/// the real pack tier). A backing failure returns `Err`, aborting the push (the ref never moves over
/// un-durable objects).
///
/// The migration writes objects as git **blob** objects (the quarantine carries opaque bytes + an
/// oid; the real kind comes from the object header the canonical-`git` quarantine produced — on this
/// floor the byte-identity round-trip is what matters, so the bytes are stored verbatim under the
/// oid handle).
pub struct PackTierMigration<'a, B: BlobStore> {
    db: &'a PackObjectDb<B>,
}

impl<'a, B: BlobStore> PackTierMigration<'a, B> {
    /// Build the migration sink over a git object DB.
    pub fn new(db: &'a PackObjectDb<B>) -> Self {
        Self { db }
    }
}

impl<B: BlobStore> QuarantineMigration for PackTierMigration<'_, B> {
    fn migrate(&self, objects: &[QuarantineObject]) -> Result<(), String> {
        for o in objects {
            // Store THROUGH the trait (content-addressed) — durable on the backing before the ack.
            // The bytes are the object body; the byte-identity round-trip (the clone GATE) is over
            // these exact bytes. A backing failure aborts the push (the ref never moves).
            self.db
                .put_object(GitObjectKind::Blob, &o.oid, &o.bytes)
                .map_err(|e| format!("pack-tier migration failed for {}: {e}", o.oid.0))?;
        }
        Ok(())
    }
}

// ───────────────────────────── the residency-pin lint (relocatable, never node-pinned) ───────────

/// **The residency-pin lint, green on the pack placement** (STOR-5 / inherited non-negotiable 8 —
/// repos RELOCATABLE, never node-pinned). Asserts a repo's pack placement is region-pinned (carries
/// a `region`) and **relocatable** (carries a storage `group`, a stored fact) and — the load-bearing
/// half — carries **NO node id / node hash** by construction. Returns `Ok(())` iff the placement is
/// well-formed; the type system already forbids a node field on [`RepoGitPlacement`], so this is the
/// positive assertion the CI lint runs (the negative — a node-pinned placement — is unrepresentable).
pub fn assert_relocatable_never_node_pinned(
    placement: &RepoGitPlacement,
) -> Result<(), ResidencyPinViolation> {
    // Region-pinned: a placement without a region would let a repo's residency drift (forbidden).
    if placement.region.as_str().is_empty() {
        return Err(ResidencyPinViolation::NoRegion);
    }
    // Relocatable: the group is the stored, within-region relocation handle (a stored fact, NOT a
    // node hash). An empty group would mean an un-relocatable (effectively node-frozen) placement.
    if placement.group.as_str().is_empty() {
        return Err(ResidencyPinViolation::NoRelocationGroup);
    }
    // The node-pin is UNREPRESENTABLE: `RepoGitPlacement` has no node id / node hash field by
    // construction (storage §3.5 / 12.2). The lint passes because the type makes a node-pin
    // impossible — the property is enforced by the data model, asserted positively here.
    Ok(())
}

/// **The residency-pin lint violation** (a malformed pack placement). PII-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidencyPinViolation {
    /// The placement carries no region — a repo's residency would drift (the pin is the region).
    NoRegion,
    /// The placement carries no relocation group — effectively node-frozen (un-relocatable).
    NoRelocationGroup,
}

impl std::fmt::Display for ResidencyPinViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResidencyPinViolation::NoRegion => write!(
                f,
                "residency-pin lint: a git pack placement MUST carry a region (the residency pin); \
                 a repo's packs are region-pinned (STOR-5)"
            ),
            ResidencyPinViolation::NoRelocationGroup => write!(
                f,
                "residency-pin lint: a git pack placement MUST carry a relocation group (relocatable \
                 within-region, never node-pinned — STOR-5)"
            ),
        }
    }
}

impl std::error::Error for ResidencyPinViolation {}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::{BlobError, FsBlobStore, RepoPlacementStatus, StorageGroup};
    use myelin_tenancy::{Region, TenantId};

    fn placed_db() -> PackObjectDb<FsBlobStore> {
        let tier = GitPackTier::new(TenantId("acme".into()), FsBlobStore::new());
        let repo = RepoId::from_token("widgets");
        tier.place_repo(
            repo.clone(),
            RepoGitPlacement {
                group: StorageGroup::from_token("pack-0"),
                region: Region::new("fr-par"),
                status: RepoPlacementStatus::Active,
            },
        );
        PackObjectDb::new(tier, repo)
    }

    /// **An object migrated through the pack tier round-trips byte-identical, addressed by its
    /// content (never a node path).** The genuinely-new git-side migration THROUGH the trait.
    #[test]
    fn object_migrates_through_the_trait_and_round_trips_byte_identical() {
        let db = placed_db();
        let oid = Oid::new("aaaa1111");
        let content = b"fn main() { println!(\"hi\"); }\n";
        let address = db.put_object(GitObjectKind::Blob, &oid, content).expect("put");
        assert!(address.to_multihash_string().starts_with("sha256:"));

        // read by the git oid returns the EXACT bytes (re-hash-on-read verified).
        assert_eq!(db.read_object(&oid).expect("read"), content);
        // the oid resolves to a CONTENT address (not a node path).
        assert_eq!(db.address_of(&oid), Some(address));
    }

    /// **THE GIT-P11 GATE: a clone served from the local-NVMe pack tier round-trips byte-identical to
    /// the receive-pack input (0 corruption).** The objects migrate in via the QuarantineMigration,
    /// then a clone serves them back byte-for-byte.
    #[test]
    fn clone_round_trips_byte_identical_to_receive_pack_input() {
        let db = placed_db();
        // The receive-pack input: a set of quarantine objects (oid + bytes).
        let input = vec![
            QuarantineObject { oid: Oid::new("c0ffee01"), bytes: b"tree-bytes-1".to_vec() },
            QuarantineObject { oid: Oid::new("c0ffee02"), bytes: b"commit-bytes-2".to_vec() },
            QuarantineObject { oid: Oid::new("c0ffee03"), bytes: vec![0u8, 1, 2, 3, 255, 254] },
        ];

        // Accept → migrate through the real pack tier (closing the receive_pack GIT-P11 floor).
        let migration = PackTierMigration::new(&db);
        migration.migrate(&input).expect("migration acked durable");

        // Clone: serve the tips back from the pack tier.
        let tips: Vec<Oid> = input.iter().map(|o| o.oid.clone()).collect();
        let served = db.serve_clone(&tips).expect("clone served");

        // BYTE-IDENTICAL: each served object equals the receive-pack input bytes (0 corruption).
        assert_eq!(served.len(), input.len());
        for (got, want) in served.iter().zip(input.iter()) {
            assert_eq!(got.0, want.oid, "the same oid is served");
            assert_eq!(got.1, want.bytes, "byte-identical clone round-trip (0 corruption)");
        }
    }

    /// **A backing failure during migration aborts the push** (the ref never moves over un-durable
    /// objects). An unplaced repo's migration is refused — the fail-closed property the receive-pack
    /// path relies on.
    #[test]
    fn migration_on_unplaced_repo_is_refused_aborting_the_push() {
        // A tier with the repo NOT placed → put_object is fail-closed.
        let tier = GitPackTier::new(TenantId("acme".into()), FsBlobStore::new());
        let db = PackObjectDb::new(tier, RepoId::from_token("ghost"));
        let migration = PackTierMigration::new(&db);
        let err = migration
            .migrate(&[QuarantineObject { oid: Oid::new("x"), bytes: vec![1] }])
            .expect_err("an unplaced repo aborts the migration (fail-closed)");
        assert!(err.contains("pack-tier migration failed"), "{err}");
    }

    /// **A corrupt object is REFUSED on clone (0 silent serve — STOR-D7 on the clone path).** The
    /// pack tier's re-hash-on-read catches the tamper; the clone never serves wrong bytes.
    #[test]
    fn corrupt_object_is_refused_on_clone_zero_silent_serve() {
        let db = placed_db();
        let oid = Oid::new("deadbeef");
        let content = b"authoritative object bytes";
        let address = db.put_object(GitObjectKind::Blob, &oid, content).expect("put");

        // Clean clone serves the bytes.
        assert_eq!(db.serve_clone(std::slice::from_ref(&oid)).unwrap()[0].1, content);
        assert_eq!(db.tier().blobs().telemetry().blob_integrity_fail(), 0);

        // Corrupt the stored object at its native blob address.
        let native = db
            .tier()
            .native_addr_for_test(&db.repo, &address)
            .expect("linked native address");
        assert!(db.tier().blobs().corrupt_for_drill(db.tier().tenant(), &native));

        // The clone REFUSES the corrupt object (never a silent wrong-bytes clone).
        match db.serve_clone(&[oid]) {
            Err(GitPackError::Blob(BlobError::IntegrityFail { .. })) => {}
            Ok(b) => panic!("SILENT WRONG-BYTES CLONE — STOR-D7 breached on the clone path: {b:?}"),
            other => panic!("expected IntegrityFail, got {other:?}"),
        }
        assert_eq!(
            db.tier().blobs().telemetry().blob_integrity_fail(),
            1,
            "a corrupt clone read must increment blob_integrity_fail (0 silent serve)"
        );
    }

    /// **Acceleration structures: a ref-update burst marks the commit-graph/bitmaps/MIDX STALE;
    /// maintenance refreshes the freshness fence.** The §8 monitored staleness signal — the
    /// load-bearing maintenance property.
    #[test]
    fn ref_update_marks_accel_stale_and_maintenance_refreshes_the_fence() {
        let db = placed_db();
        // No artifacts yet → all three are stale (must be built).
        for k in AccelKind::all() {
            assert!(db.is_stale(k), "a missing {k:?} is stale (must be built)");
        }

        // Build the acceleration structures fresh (the post-push maintenance pass).
        db.run_maintenance(|kind| format!("{kind:?}-artifact-bytes").into_bytes())
            .expect("maintenance");
        for k in AccelKind::all() {
            assert!(!db.is_stale(k), "after maintenance {k:?} is fresh");
            let a = db.accel_artifact(k).expect("built");
            assert_eq!(a.fresh_at_fence, db.generation(), "fresh at the current generation");
        }

        // A NEW push (an object migration) advances the generation → the artifacts are stale again.
        let gen_before = db.generation();
        db.put_object(GitObjectKind::Commit, &Oid::new("newtip"), b"new commit")
            .expect("put");
        assert!(db.generation() > gen_before, "a push advances the object-DB generation");
        for k in AccelKind::all() {
            assert!(db.is_stale(k), "the ref-update burst marked {k:?} stale (the §8 signal)");
        }

        // A maintenance re-run refreshes the fence (the artifacts are fresh again).
        db.run_maintenance(|kind| format!("{kind:?}-refreshed").into_bytes())
            .expect("maintenance re-run");
        for k in AccelKind::all() {
            assert!(!db.is_stale(k), "maintenance refreshed {k:?}");
        }
    }

    /// Each acceleration kind maps to its producing canonical-`git` maintenance wire op (the byte
    /// production routes to the sandboxed Shell backend — never produced in-process).
    #[test]
    fn accel_kinds_map_to_their_canonical_git_maintenance_op() {
        use crate::core::{backend_for, Backend, GitOp, Maintenance};
        for k in AccelKind::all() {
            let m: Maintenance = k.producing_maintenance();
            assert_eq!(
                backend_for(GitOp::Maint(m)),
                Backend::Shell,
                "{k:?} byte production is a sandboxed canonical-git wire op"
            );
        }
        assert_eq!(
            AccelKind::CommitGraph.producing_maintenance(),
            Maintenance::WriteCommitGraph
        );
        assert_eq!(AccelKind::Bitmaps.producing_maintenance(), Maintenance::WriteBitmaps);
        assert_eq!(AccelKind::Midx.producing_maintenance(), Maintenance::WriteMidx);
    }

    /// **The acceleration artifact is content-addressed + relocation-stable: relocating the repo
    /// within its region does NOT move or re-address it.** The artifact rides the same content tier
    /// as objects, so it relocates with the repo (never node-pinned).
    #[test]
    fn accel_artifact_is_content_addressed_and_survives_relocation() {
        let db = placed_db();
        let artifact = db
            .record_maintenance(AccelKind::CommitGraph, b"commit-graph bytes")
            .expect("recorded");
        assert!(artifact.blob.to_multihash_string().starts_with("blake3:"));

        // Relocate the repo within its region (the group flips; no address recompute).
        db.tier()
            .relocate(&db.repo, StorageGroup::from_token("pack-9"), &Region::new("fr-par"))
            .expect("same-region relocation admitted");

        // The artifact is still served by the SAME content address (relocation-stable).
        let after = db.accel_artifact(AccelKind::CommitGraph).expect("still present");
        assert_eq!(after.blob, artifact.blob, "the artifact's address is unchanged by relocation");
        assert_eq!(
            db.tier().get_pack(&db.repo, &after.blob).expect("served after relocation"),
            b"commit-graph bytes"
        );
    }

    /// **The residency-pin lint is GREEN on a well-formed pack placement** (region-pinned +
    /// relocatable + no node id by construction) and **REJECTS** a malformed one (no region / no
    /// relocation group). The node-pin is unrepresentable — the type forbids it.
    #[test]
    fn residency_pin_lint_green_on_placement_rejects_malformed() {
        let good = RepoGitPlacement {
            group: StorageGroup::from_token("pack-0"),
            region: Region::new("fr-par"),
            status: RepoPlacementStatus::Active,
        };
        assert!(assert_relocatable_never_node_pinned(&good).is_ok());

        // No region → the residency pin is missing (the repo's residency would drift).
        let no_region = RepoGitPlacement {
            group: StorageGroup::from_token("pack-0"),
            region: Region::new(""),
            status: RepoPlacementStatus::Active,
        };
        assert_eq!(
            assert_relocatable_never_node_pinned(&no_region),
            Err(ResidencyPinViolation::NoRegion)
        );

        // No relocation group → effectively node-frozen (un-relocatable).
        let no_group = RepoGitPlacement {
            group: StorageGroup::from_token(""),
            region: Region::new("fr-par"),
            status: RepoPlacementStatus::Active,
        };
        assert_eq!(
            assert_relocatable_never_node_pinned(&no_group),
            Err(ResidencyPinViolation::NoRelocationGroup)
        );
    }

    /// The lint also passes on the LIVE placement of a placed object DB (the integration of the
    /// lint with the real pack tier — the CI residency-pin signal on the pack placement).
    #[test]
    fn residency_pin_lint_green_on_live_pack_placement() {
        let db = placed_db();
        let placement = db.placement().expect("placed");
        assert!(
            assert_relocatable_never_node_pinned(&placement).is_ok(),
            "the residency-pin lint is green on the live pack placement"
        );
    }

    /// The violation errors render loud + specific (a refusal is diagnosable — EI-01 §3).
    #[test]
    fn residency_pin_violations_display_loud() {
        assert!(ResidencyPinViolation::NoRegion.to_string().contains("region"));
        assert!(ResidencyPinViolation::NoRelocationGroup
            .to_string()
            .contains("relocatable"));
        assert_ne!(
            ResidencyPinViolation::NoRegion.to_string(),
            ResidencyPinViolation::NoRelocationGroup.to_string()
        );
    }
}

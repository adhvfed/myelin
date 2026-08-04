use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_storage::{
    BlobStore, ContentHash, GitObjectKind, GitPackError, GitPackTier, RepoGitPlacement, RepoId,
    GIT_PACK_OBJECT_MAX_STORED_BYTES,
};

use crate::receive_pack::{Oid, QuarantineMigration, QuarantineObject};

const CLONE_MAX_TIPS: usize = 100_000;
const CLONE_MAX_TOTAL_STORED_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AccelKind {
    CommitGraph,
    Bitmaps,
    Midx,
}

impl AccelKind {
    pub fn producing_maintenance(self) -> crate::core::Maintenance {
        match self {
            AccelKind::CommitGraph => crate::core::Maintenance::WriteCommitGraph,
            AccelKind::Bitmaps => crate::core::Maintenance::WriteBitmaps,
            AccelKind::Midx => crate::core::Maintenance::WriteMidx,
        }
    }

    pub fn all() -> [AccelKind; 3] {
        [AccelKind::CommitGraph, AccelKind::Bitmaps, AccelKind::Midx]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccelArtifact {
    pub blob: ContentHash,
    pub fresh_at_fence: u64,
}

pub struct PackObjectDb<B: BlobStore> {
    tier: GitPackTier<B>,
    repo: RepoId,
    oid_index: Mutex<BTreeMap<String, ContentHash>>,
    generation: Mutex<u64>,
    accel: Mutex<BTreeMap<AccelKind, AccelArtifact>>,
}

impl<B: BlobStore> PackObjectDb<B> {
    pub fn new(tier: GitPackTier<B>, repo: RepoId) -> Self {
        Self {
            tier,
            repo,
            oid_index: Mutex::new(BTreeMap::new()),
            generation: Mutex::new(0),
            accel: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn tier(&self) -> &GitPackTier<B> {
        &self.tier
    }

    pub fn repo(&self) -> &RepoId {
        &self.repo
    }

    pub fn placement(&self) -> Option<RepoGitPlacement> {
        self.tier.placement_of(&self.repo)
    }

    pub fn generation(&self) -> u64 {
        *self.generation.lock().expect("generation mutex")
    }

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
        *self.generation.lock().expect("generation mutex") += 1;
        Ok(address)
    }

    pub fn read_object(&self, oid: &Oid) -> Result<Vec<u8>, GitPackError> {
        self.read_object_bounded(oid, GIT_PACK_OBJECT_MAX_STORED_BYTES)
    }

    pub fn read_object_bounded(
        &self,
        oid: &Oid,
        maximum_stored_bytes: usize,
    ) -> Result<Vec<u8>, GitPackError> {
        let address = self
            .address_of(oid)
            .ok_or_else(|| GitPackError::RepoNotPlaced {
                repo: self.repo.clone(),
            })?;
        self.tier
            .get_object_bounded(&self.repo, &address, maximum_stored_bytes)
    }

    pub fn address_of(&self, oid: &Oid) -> Option<ContentHash> {
        self.oid_index
            .lock()
            .expect("oid index mutex")
            .get(&oid.0)
            .cloned()
    }

    pub fn record_maintenance(
        &self,
        kind: AccelKind,
        artifact_bytes: &[u8],
    ) -> Result<AccelArtifact, GitPackError> {
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

    pub fn is_stale(&self, kind: AccelKind) -> bool {
        match self.accel.lock().expect("accel mutex").get(&kind) {
            None => true,
            Some(a) => a.fresh_at_fence < self.generation(),
        }
    }

    pub fn accel_artifact(&self, kind: AccelKind) -> Option<AccelArtifact> {
        self.accel.lock().expect("accel mutex").get(&kind).cloned()
    }

    pub fn serve_clone(&self, tips: &[Oid]) -> Result<Vec<(Oid, Vec<u8>)>, GitPackError> {
        self.serve_clone_bounded(
            tips,
            CLONE_MAX_TIPS,
            GIT_PACK_OBJECT_MAX_STORED_BYTES,
            CLONE_MAX_TOTAL_STORED_BYTES,
        )
    }

    pub fn serve_clone_bounded(
        &self,
        tips: &[Oid],
        maximum_tips: usize,
        maximum_stored_bytes_per_object: usize,
        maximum_total_stored_bytes: usize,
    ) -> Result<Vec<(Oid, Vec<u8>)>, GitPackError> {
        if tips.len() > maximum_tips {
            return Err(GitPackError::ReadLimitExceeded {
                actual: tips.len(),
                maximum: maximum_tips,
            });
        }
        let mut total_stored_bytes = 0usize;
        for oid in tips {
            let address = self
                .address_of(oid)
                .ok_or_else(|| GitPackError::RepoNotPlaced {
                    repo: self.repo.clone(),
                })?;
            let stored_bytes = self.tier.object_stored_len(&self.repo, &address)?;
            if stored_bytes > maximum_stored_bytes_per_object {
                return Err(GitPackError::ReadLimitExceeded {
                    actual: stored_bytes,
                    maximum: maximum_stored_bytes_per_object,
                });
            }
            total_stored_bytes = total_stored_bytes
                .checked_add(stored_bytes)
                .ok_or(GitPackError::ReadLimitExceeded {
                    actual: usize::MAX,
                    maximum: maximum_total_stored_bytes,
                })?;
            if total_stored_bytes > maximum_total_stored_bytes {
                return Err(GitPackError::ReadLimitExceeded {
                    actual: total_stored_bytes,
                    maximum: maximum_total_stored_bytes,
                });
            }
        }
        let mut out = Vec::with_capacity(tips.len());
        for oid in tips {
            let bytes = self.read_object_bounded(oid, maximum_stored_bytes_per_object)?;
            out.push((oid.clone(), bytes));
        }
        Ok(out)
    }
}

pub struct PackTierMigration<'a, B: BlobStore> {
    db: &'a PackObjectDb<B>,
}

impl<'a, B: BlobStore> PackTierMigration<'a, B> {
    pub fn new(db: &'a PackObjectDb<B>) -> Self {
        Self { db }
    }
}

impl<B: BlobStore> QuarantineMigration for PackTierMigration<'_, B> {
    fn migrate(&self, objects: &[QuarantineObject]) -> Result<(), String> {
        for o in objects {
            self.db
                .put_object(GitObjectKind::Blob, &o.oid, &o.bytes)
                .map_err(|e| format!("pack-tier migration failed for {}: {e}", o.oid.0))?;
        }
        Ok(())
    }
}

pub fn assert_relocatable_never_node_pinned(
    placement: &RepoGitPlacement,
) -> Result<(), ResidencyPinViolation> {
    if placement.region.as_str().is_empty() {
        return Err(ResidencyPinViolation::NoRegion);
    }
    if placement.group.as_str().is_empty() {
        return Err(ResidencyPinViolation::NoRelocationGroup);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidencyPinViolation {
    NoRegion,
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
                 within-region, never node-pinned - STOR-5)"
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

    #[test]
    fn object_migrates_through_the_trait_and_round_trips_byte_identical() {
        let db = placed_db();
        let oid = Oid::new("aaaa1111");
        let content = b"fn main() { println!(\"hi\"); }\n";
        let address = db
            .put_object(GitObjectKind::Blob, &oid, content)
            .expect("put");
        assert!(address.to_multihash_string().starts_with("sha256:"));

        assert_eq!(db.read_object(&oid).expect("read"), content);
        assert_eq!(db.address_of(&oid), Some(address));
    }

    #[test]
    fn clone_round_trips_byte_identical_to_receive_pack_input() {
        let db = placed_db();
        let input = vec![
            QuarantineObject {
                oid: Oid::new("c0ffee01"),
                bytes: b"tree-bytes-1".to_vec(),
            },
            QuarantineObject {
                oid: Oid::new("c0ffee02"),
                bytes: b"commit-bytes-2".to_vec(),
            },
            QuarantineObject {
                oid: Oid::new("c0ffee03"),
                bytes: vec![0u8, 1, 2, 3, 255, 254],
            },
        ];

        let migration = PackTierMigration::new(&db);
        migration.migrate(&input).expect("migration acked durable");

        let tips: Vec<Oid> = input.iter().map(|o| o.oid.clone()).collect();
        let served = db.serve_clone(&tips).expect("clone served");

        assert_eq!(served.len(), input.len());
        for (got, want) in served.iter().zip(input.iter()) {
            assert_eq!(got.0, want.oid, "the same oid is served");
            assert_eq!(
                got.1, want.bytes,
                "byte-identical clone round-trip (0 corruption)"
            );
        }
    }

    #[test]
    fn bounded_clone_enforces_count_per_object_and_aggregate_limits() {
        let db = placed_db();
        let first = Oid::new("first");
        let second = Oid::new("second");
        let first_address = db
            .put_object(GitObjectKind::Blob, &first, b"1234")
            .expect("put first");
        let second_address = db
            .put_object(GitObjectKind::Blob, &second, b"5678")
            .expect("put second");
        let tips = [first, second];
        let stored_total = db
            .tier()
            .object_stored_len(db.repo(), &first_address)
            .expect("first metadata")
            + db
                .tier()
                .object_stored_len(db.repo(), &second_address)
                .expect("second metadata");

        assert_eq!(
            db.serve_clone_bounded(&tips, 2, 64, stored_total)
                .expect("exact limits accepted")
                .len(),
            2
        );
        assert!(db
            .serve_clone_bounded(&tips, 1, 64, stored_total)
            .is_err());
        assert!(db
            .serve_clone_bounded(&tips, 2, 1, stored_total)
            .is_err());
        assert!(db
            .serve_clone_bounded(&tips, 2, 64, stored_total - 1)
            .is_err());
    }

    #[test]
    fn migration_on_unplaced_repo_is_refused_aborting_the_push() {
        let tier = GitPackTier::new(TenantId("acme".into()), FsBlobStore::new());
        let db = PackObjectDb::new(tier, RepoId::from_token("ghost"));
        let migration = PackTierMigration::new(&db);
        let err = migration
            .migrate(&[QuarantineObject {
                oid: Oid::new("x"),
                bytes: vec![1],
            }])
            .expect_err("an unplaced repo aborts the migration (fail-closed)");
        assert!(err.contains("pack-tier migration failed"), "{err}");
    }

    #[test]
    fn corrupt_object_is_refused_on_clone_zero_silent_serve() {
        let db = placed_db();
        let oid = Oid::new("deadbeef");
        let content = b"authoritative object bytes";
        let address = db
            .put_object(GitObjectKind::Blob, &oid, content)
            .expect("put");

        assert_eq!(
            db.serve_clone(std::slice::from_ref(&oid)).unwrap()[0].1,
            content
        );
        assert_eq!(db.tier().blobs().telemetry().blob_integrity_fail(), 0);

        let native = db
            .tier()
            .native_addr_for_test(&db.repo, &address)
            .expect("linked native address");
        assert!(db
            .tier()
            .blobs()
            .corrupt_for_drill(db.tier().tenant(), &native));

        match db.serve_clone(&[oid]) {
            Err(GitPackError::Blob(BlobError::IntegrityFail { .. })) => {}
            Ok(b) => panic!("SILENT WRONG-BYTES CLONE - STOR-D7 breached on the clone path: {b:?}"),
            other => panic!("expected IntegrityFail, got {other:?}"),
        }
        assert_eq!(
            db.tier().blobs().telemetry().blob_integrity_fail(),
            1,
            "a corrupt clone read must increment blob_integrity_fail (0 silent serve)"
        );
    }

    #[test]
    fn ref_update_marks_accel_stale_and_maintenance_refreshes_the_fence() {
        let db = placed_db();
        for k in AccelKind::all() {
            assert!(db.is_stale(k), "a missing {k:?} is stale (must be built)");
        }

        db.run_maintenance(|kind| format!("{kind:?}-artifact-bytes").into_bytes())
            .expect("maintenance");
        for k in AccelKind::all() {
            assert!(!db.is_stale(k), "after maintenance {k:?} is fresh");
            let a = db.accel_artifact(k).expect("built");
            assert_eq!(
                a.fresh_at_fence,
                db.generation(),
                "fresh at the current generation"
            );
        }

        let gen_before = db.generation();
        db.put_object(GitObjectKind::Commit, &Oid::new("newtip"), b"new commit")
            .expect("put");
        assert!(
            db.generation() > gen_before,
            "a push advances the object-DB generation"
        );
        for k in AccelKind::all() {
            assert!(
                db.is_stale(k),
                "the ref-update burst marked {k:?} stale (the §8 signal)"
            );
        }

        db.run_maintenance(|kind| format!("{kind:?}-refreshed").into_bytes())
            .expect("maintenance re-run");
        for k in AccelKind::all() {
            assert!(!db.is_stale(k), "maintenance refreshed {k:?}");
        }
    }

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
        assert_eq!(
            AccelKind::Bitmaps.producing_maintenance(),
            Maintenance::WriteBitmaps
        );
        assert_eq!(
            AccelKind::Midx.producing_maintenance(),
            Maintenance::WriteMidx
        );
    }

    #[test]
    fn accel_artifact_is_content_addressed_and_survives_relocation() {
        let db = placed_db();
        let artifact = db
            .record_maintenance(AccelKind::CommitGraph, b"commit-graph bytes")
            .expect("recorded");
        assert!(artifact.blob.to_multihash_string().starts_with("blake3:"));

        db.tier()
            .relocate(
                &db.repo,
                StorageGroup::from_token("pack-9"),
                &Region::new("fr-par"),
            )
            .expect("same-region relocation admitted");

        let after = db
            .accel_artifact(AccelKind::CommitGraph)
            .expect("still present");
        assert_eq!(
            after.blob, artifact.blob,
            "the artifact's address is unchanged by relocation"
        );
        assert_eq!(
            db.tier()
                .get_pack(&db.repo, &after.blob)
                .expect("served after relocation"),
            b"commit-graph bytes"
        );
    }

    #[test]
    fn residency_pin_lint_green_on_placement_rejects_malformed() {
        let good = RepoGitPlacement {
            group: StorageGroup::from_token("pack-0"),
            region: Region::new("fr-par"),
            status: RepoPlacementStatus::Active,
        };
        assert!(assert_relocatable_never_node_pinned(&good).is_ok());

        let no_region = RepoGitPlacement {
            group: StorageGroup::from_token("pack-0"),
            region: Region::new(""),
            status: RepoPlacementStatus::Active,
        };
        assert_eq!(
            assert_relocatable_never_node_pinned(&no_region),
            Err(ResidencyPinViolation::NoRegion)
        );

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

    #[test]
    fn residency_pin_lint_green_on_live_pack_placement() {
        let db = placed_db();
        let placement = db.placement().expect("placed");
        assert!(
            assert_relocatable_never_node_pinned(&placement).is_ok(),
            "the residency-pin lint is green on the live pack placement"
        );
    }

    #[test]
    fn residency_pin_violations_display_loud() {
        assert!(ResidencyPinViolation::NoRegion
            .to_string()
            .contains("region"));
        assert!(ResidencyPinViolation::NoRelocationGroup
            .to_string()
            .contains("relocatable"));
        assert_ne!(
            ResidencyPinViolation::NoRegion.to_string(),
            ResidencyPinViolation::NoRelocationGroup.to_string()
        );
    }
}

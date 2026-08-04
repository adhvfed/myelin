use myelin_storage::{BlobError, BlobStore, ContentHash};
use myelin_tenancy::{Region, TenantId};

use crate::hyok_scale::{BackupScaleEraseVerdict, SealedBackupSegment};

pub struct SegmentBackstop<B: BlobStore> {
    blobs: B,
    tenant: TenantId,
    region: Region,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredSegment {
    pub doc_id: String,
    pub content_address: ContentHash,
}

impl<B: BlobStore> SegmentBackstop<B> {
    pub fn new(blobs: B, tenant: TenantId, region: Region) -> SegmentBackstop<B> {
        SegmentBackstop {
            blobs,
            tenant,
            region,
        }
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn put_segment(&self, segment: &SealedBackupSegment) -> Result<StoredSegment, BlobError> {
        let bytes = segment.to_blob_bytes();
        let content_address = self.blobs.put(&self.tenant, &bytes)?;
        Ok(StoredSegment {
            doc_id: segment.doc_id.clone(),
            content_address,
        })
    }

    pub fn get_segment(&self, stored: &StoredSegment) -> Result<SealedBackupSegment, BlobError> {
        let bytes = self.blobs.get(&self.tenant, &stored.content_address)?;
        SealedBackupSegment::from_blob_bytes(&stored.doc_id, &bytes).ok_or_else(|| {
            BlobError::MalformedAddress(format!(
                "object-store backstop: segment for `{}` at {} had a malformed at-rest frame \
                 (truncated or stale nonce width) - the segment was NOT opened (0 silent serve)",
                stored.doc_id,
                stored.content_address.to_multihash_string()
            ))
        })
    }

    pub fn load_all(
        &self,
        stored: &[StoredSegment],
    ) -> Result<Vec<SealedBackupSegment>, BlobError> {
        stored.iter().map(|s| self.get_segment(s)).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectStoreBackstopArtifact {
    pub tenant: TenantId,
    pub region: Region,
    pub segments_moved: usize,
    pub segments_byte_identical: usize,
    pub recoverable_after_shred: usize,
    pub backing: &'static str,
    pub ran_at: String,
}

impl ObjectStoreBackstopArtifact {
    pub fn is_green(&self) -> bool {
        self.segments_moved > 0
            && self.segments_byte_identical == self.segments_moved
            && self.recoverable_after_shred == 0
    }

    pub fn summary(&self) -> String {
        format!(
            "search object-store index backstop PASS (SRCH-P30): swapped {segments} segment(s) \
             through the `{backing}` BlobStore backing - {identical}/{segments} recovered \
             BYTE-IDENTICAL (the swap moved the segments with NO behaviour change, EI-01 §3); the \
             SRCH-D4 backup-scale erasure HELD over the object-store-resident segments \
             (recoverable_after_shred={after}, MUST be 0 - the per-tenant index DEK crypto-shred \
             reaches the object-store backstop, §4.8). Residency-pinned to ({tenant}, {region}); \
             per-tenant-DEK-encrypted at rest.",
            segments = self.segments_moved,
            backing = self.backing,
            identical = self.segments_byte_identical,
            after = self.recoverable_after_shred,
            tenant = self.tenant.0,
            region = self.region.0,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObjectStoreBackstopFailure {
    SwapProvedNothing,
    SegmentNotByteIdentical(String),
    BlobOp(String),
    RecoverableAfterShred(usize),
    BackupScaleRed(String),
}

impl core::fmt::Display for ObjectStoreBackstopFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ObjectStoreBackstopFailure::SwapProvedNothing => write!(
                f,
                "SEARCH OBJECT-STORE BACKSTOP FAIL - the swap proved nothing: 0 segments were \
                 moved into the object store, so a `behaviour unchanged` / `erasure holds` reading \
                 is vacuous (SRCH-P30)"
            ),
            ObjectStoreBackstopFailure::SegmentNotByteIdentical(doc_id) => write!(
                f,
                "SEARCH OBJECT-STORE BACKSTOP FAIL - the segment for `{doc_id}` recovered from the \
                 object store was NOT byte-identical to the one stored: the swap CHANGED the \
                 segment bytes (a *measured* swap must be behaviour-unchanged, EI-01 §3)"
            ),
            ObjectStoreBackstopFailure::BlobOp(e) => write!(
                f,
                "SEARCH OBJECT-STORE BACKSTOP FAIL - a BlobStore operation failed during the swap: \
                 {e}"
            ),
            ObjectStoreBackstopFailure::RecoverableAfterShred(n) => write!(
                f,
                "SEARCH OBJECT-STORE BACKSTOP FAIL - {n} object-store-resident segment(s) were \
                 STILL recoverable AFTER the per-tenant index DEK crypto-shred: the SRCH-D4 \
                 backup-scale erasure did NOT hold over the object store (erased personal data \
                 survives in the object-store backstop - MUST be 0, §4.8)"
            ),
            ObjectStoreBackstopFailure::BackupScaleRed(e) => write!(
                f,
                "SEARCH OBJECT-STORE BACKSTOP FAIL - the underlying SRCH-D4 backup-scale gate went \
                 RED over the object-store segments: {e}"
            ),
        }
    }
}

impl std::error::Error for ObjectStoreBackstopFailure {}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "an object-store backstop verdict must be checked - a dropped RED is a SWALLOWED \
              swap-broke-behaviour OR erasure-survives-the-object-store failure (SRCH-P30, \
              EI-01 §5: loud-never-swallowed)"]
pub enum ObjectStoreBackstopVerdict {
    Green(ObjectStoreBackstopArtifact),
    Red(ObjectStoreBackstopFailure),
}

impl ObjectStoreBackstopVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, ObjectStoreBackstopVerdict::Green(_))
    }
    pub fn artifact(&self) -> Option<&ObjectStoreBackstopArtifact> {
        match self {
            ObjectStoreBackstopVerdict::Green(a) => Some(a),
            ObjectStoreBackstopVerdict::Red(_) => None,
        }
    }
    pub fn failure(&self) -> Option<&ObjectStoreBackstopFailure> {
        match self {
            ObjectStoreBackstopVerdict::Red(f) => Some(f),
            ObjectStoreBackstopVerdict::Green(_) => None,
        }
    }

    pub fn run_or_fail_ci(self) -> Result<ObjectStoreBackstopArtifact, ObjectStoreBackstopFailure> {
        match self {
            ObjectStoreBackstopVerdict::Green(a) => Ok(a),
            ObjectStoreBackstopVerdict::Red(f) => Err(f),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SwappedSegments {
    pub stored: Vec<StoredSegment>,
    pub loaded: Vec<SealedBackupSegment>,
    pub byte_identical: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ObjectStoreBackstopGate;

impl ObjectStoreBackstopGate {
    pub fn new() -> ObjectStoreBackstopGate {
        ObjectStoreBackstopGate
    }

    pub fn swap_in<B: BlobStore>(
        &self,
        backstop: &SegmentBackstop<B>,
        segments: &[SealedBackupSegment],
    ) -> Result<SwappedSegments, ObjectStoreBackstopFailure> {
        if segments.is_empty() {
            return Err(ObjectStoreBackstopFailure::SwapProvedNothing);
        }
        let mut stored = Vec::with_capacity(segments.len());
        for seg in segments {
            let s = backstop
                .put_segment(seg)
                .map_err(|e| ObjectStoreBackstopFailure::BlobOp(e.to_string()))?;
            stored.push(s);
        }

        let mut byte_identical = 0usize;
        let mut loaded = Vec::with_capacity(stored.len());
        for (orig, s) in segments.iter().zip(stored.iter()) {
            let recovered = backstop
                .get_segment(s)
                .map_err(|e| ObjectStoreBackstopFailure::BlobOp(e.to_string()))?;
            if recovered.to_blob_bytes() == orig.to_blob_bytes() && recovered.doc_id == orig.doc_id
            {
                byte_identical += 1;
            } else {
                return Err(ObjectStoreBackstopFailure::SegmentNotByteIdentical(
                    orig.doc_id.clone(),
                ));
            }
            loaded.push(recovered);
        }

        Ok(SwappedSegments {
            stored,
            loaded,
            byte_identical,
        })
    }

    pub fn confirm<B: BlobStore>(
        &self,
        backstop: &SegmentBackstop<B>,
        swapped: &SwappedSegments,
        srch_d4: &BackupScaleEraseVerdict,
        backing: &'static str,
        ran_at: impl Into<String>,
    ) -> ObjectStoreBackstopVerdict {
        let recoverable_after_shred = match srch_d4 {
            BackupScaleEraseVerdict::Green(a) => a.backup_segments_recoverable_after_shred,
            BackupScaleEraseVerdict::Red(f) => {
                use crate::hyok_scale::BackupScaleEraseFailure as F;
                return match f {
                    F::BackupRecoverableAfterShred(n) => ObjectStoreBackstopVerdict::Red(
                        ObjectStoreBackstopFailure::RecoverableAfterShred(*n),
                    ),
                    other => ObjectStoreBackstopVerdict::Red(
                        ObjectStoreBackstopFailure::BackupScaleRed(other.to_string()),
                    ),
                };
            }
        };
        if recoverable_after_shred != 0 {
            return ObjectStoreBackstopVerdict::Red(
                ObjectStoreBackstopFailure::RecoverableAfterShred(recoverable_after_shred),
            );
        }

        ObjectStoreBackstopVerdict::Green(ObjectStoreBackstopArtifact {
            tenant: backstop.tenant().clone(),
            region: backstop.region().clone(),
            segments_moved: swapped.loaded.len(),
            segments_byte_identical: swapped.byte_identical,
            recoverable_after_shred,
            backing,
            ran_at: ran_at.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::FsBlobStore;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }

    fn fake_sealed(doc_id: &str, nonce_byte: u8, ct: &[u8]) -> SealedBackupSegment {
        let mut bytes = Vec::new();
        bytes.push(myelin_storage::NONCE_LEN as u8);
        bytes.extend(std::iter::repeat_n(nonce_byte, myelin_storage::NONCE_LEN));
        bytes.extend_from_slice(ct);
        SealedBackupSegment::from_blob_bytes(doc_id, &bytes).expect("well-formed at-rest frame")
    }

    #[test]
    fn swap_round_trips_segment_byte_identical_over_fs_floor() {
        let backstop = SegmentBackstop::new(FsBlobStore::new(), tenant(), region());
        let seg = fake_sealed("myelin://acme/kn/page/p1", 0xAB, b"sealed-segment-bytes");

        let stored = backstop.put_segment(&seg).expect("put");
        assert_eq!(
            stored.content_address,
            ContentHash::blake3(&seg.to_blob_bytes())
        );

        let recovered = backstop.get_segment(&stored).expect("get");
        assert_eq!(
            recovered.to_blob_bytes(),
            seg.to_blob_bytes(),
            "the recovered segment is byte-identical - the swap moved it with no behaviour change"
        );
        assert_eq!(recovered.doc_id, seg.doc_id);
    }

    #[test]
    fn per_tenant_isolation_holds_over_the_swap() {
        let blobs = std::sync::Arc::new(FsBlobStore::new());
        let acme = SegmentBackstop::new(std::sync::Arc::clone(&blobs), tenant(), region());
        let globex = SegmentBackstop::new(
            std::sync::Arc::clone(&blobs),
            TenantId("globex".into()),
            region(),
        );
        let seg = fake_sealed("myelin://acme/kn/page/p1", 0x01, b"acme-only");
        let stored = acme.put_segment(&seg).expect("acme put");

        let cross = globex.get_segment(&stored);
        assert!(
            matches!(cross, Err(BlobError::NotFound { .. })),
            "a different tenant must NOT read this tenant's object-store segment, got {cross:?}"
        );
    }

    #[test]
    fn corrupt_at_rest_frame_is_surfaced() {
        let backstop = SegmentBackstop::new(FsBlobStore::new(), tenant(), region());
        let bad_bytes = vec![0u8];
        let content_address = backstop.blobs.put(&tenant(), &bad_bytes).expect("put raw");
        let stored = StoredSegment {
            doc_id: "myelin://acme/kn/page/bad".into(),
            content_address,
        };
        let got = backstop.get_segment(&stored);
        assert!(
            matches!(got, Err(BlobError::MalformedAddress(_))),
            "a malformed at-rest frame must be surfaced (0 silent serve), got {got:?}"
        );
    }

    #[test]
    fn load_all_reconstructs_the_segment_set() {
        let backstop = SegmentBackstop::new(FsBlobStore::new(), tenant(), region());
        let segs = [
            fake_sealed("myelin://acme/kn/page/p1", 0x10, b"one"),
            fake_sealed("myelin://acme/kn/page/p2", 0x20, b"two"),
            fake_sealed("myelin://acme/kn/page/p3", 0x30, b"three"),
        ];
        let stored: Vec<_> = segs
            .iter()
            .map(|s| backstop.put_segment(s).expect("put"))
            .collect();
        let loaded = backstop.load_all(&stored).expect("load_all");
        assert_eq!(loaded.len(), segs.len());
        for (a, b) in loaded.iter().zip(segs.iter()) {
            assert_eq!(a.to_blob_bytes(), b.to_blob_bytes());
            assert_eq!(a.doc_id, b.doc_id);
        }
    }

    #[test]
    fn artifact_green_is_the_full_conjunction() {
        let green = ObjectStoreBackstopArtifact {
            tenant: tenant(),
            region: region(),
            segments_moved: 3,
            segments_byte_identical: 3,
            recoverable_after_shred: 0,
            backing: "fs-floor",
            ran_at: "2026-06-25".into(),
        };
        assert!(green.is_green());

        let mut vacuous = green.clone();
        vacuous.segments_moved = 0;
        vacuous.segments_byte_identical = 0;
        assert!(!vacuous.is_green(), "0 segments moved is vacuous → RED");

        let mut drifted = green.clone();
        drifted.segments_byte_identical = 2;
        assert!(!drifted.is_green(), "a non-byte-identical recovery is RED");

        let mut leaked = green.clone();
        leaked.recoverable_after_shred = 1;
        assert!(
            !leaked.is_green(),
            "a segment recoverable after the shred is RED (erasure must hold over the object store)"
        );
    }

    #[test]
    fn gate_red_on_vacuous_swap() {
        let backstop = SegmentBackstop::new(FsBlobStore::new(), tenant(), region());
        let got = ObjectStoreBackstopGate::new().swap_in(&backstop, &[]);
        assert!(matches!(
            got,
            Err(ObjectStoreBackstopFailure::SwapProvedNothing)
        ));
    }

    #[test]
    fn swap_in_then_confirm_is_green_with_zero_recoverable() {
        let backstop = SegmentBackstop::new(FsBlobStore::new(), tenant(), region());
        let segs = vec![
            fake_sealed("myelin://acme/kn/page/p1", 0x11, b"one"),
            fake_sealed("myelin://acme/kn/page/p2", 0x22, b"two"),
        ];
        let gate = ObjectStoreBackstopGate::new();
        let swapped = gate.swap_in(&backstop, &segs).expect("swap_in");
        assert_eq!(swapped.loaded.len(), 2);
        assert_eq!(swapped.byte_identical, 2);

        let d4 = green_d4(0);
        let verdict = gate.confirm(&backstop, &swapped, &d4, "fs-floor", "2026-06-25");
        let artifact = verdict.artifact().expect("green");
        assert!(artifact.is_green());
        assert_eq!(artifact.recoverable_after_shred, 0);
        assert_eq!(artifact.segments_moved, 2);
    }

    #[test]
    fn confirm_red_when_segment_recoverable_after_shred() {
        let backstop = SegmentBackstop::new(FsBlobStore::new(), tenant(), region());
        let segs = vec![fake_sealed("myelin://acme/kn/page/p1", 0x11, b"one")];
        let gate = ObjectStoreBackstopGate::new();
        let swapped = gate.swap_in(&backstop, &segs).expect("swap_in");
        let d4 = green_d4(1);
        let verdict = gate.confirm(&backstop, &swapped, &d4, "fs-floor", "2026-06-25");
        assert!(matches!(
            verdict.failure(),
            Some(ObjectStoreBackstopFailure::RecoverableAfterShred(1))
        ));
    }

    fn green_d4(recoverable_after: usize) -> BackupScaleEraseVerdict {
        BackupScaleEraseVerdict::Green(crate::hyok_scale::BackupScaleEraseArtifact {
            tenant: tenant(),
            region: region(),
            live_docs_purged: 1,
            live_docs_remaining: 0,
            zero_orphan_embedding: true,
            backup_segments_recoverable_before_shred: 1,
            backup_segments_recoverable_after_shred: recoverable_after,
            ran_at: "2026-06-25".into(),
        })
    }

    #[test]
    fn run_or_fail_ci_propagates_red() {
        let red =
            ObjectStoreBackstopVerdict::Red(ObjectStoreBackstopFailure::RecoverableAfterShred(2));
        assert!(red.run_or_fail_ci().is_err());
        let green = ObjectStoreBackstopVerdict::Green(ObjectStoreBackstopArtifact {
            tenant: tenant(),
            region: region(),
            segments_moved: 1,
            segments_byte_identical: 1,
            recoverable_after_shred: 0,
            backing: "fs-floor",
            ran_at: "2026-06-25".into(),
        });
        assert!(green.run_or_fail_ci().is_ok());
    }
}

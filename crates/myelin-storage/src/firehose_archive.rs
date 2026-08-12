use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use myelin_events::{Frame, FrameDraft, FramePayload};
use myelin_gdpr::ErasureMethod;
use myelin_tenancy::{Region, TenantId};

use crate::blob::{BlobError, BlobStore, ContentHash, FsBlobStore};
use crate::encryption::DekContentWrap;
use crate::kms::KmsEngine;
use crate::residency::{ResidencyStoreClass, StoreResidencyReport};

pub const FIREHOSE_MAX_SEGMENT_BYTES: usize = 64 * 1024 * 1024;
pub const FIREHOSE_MAX_STORED_SEGMENT_BYTES: usize = FIREHOSE_MAX_SEGMENT_BYTES + 1024 * 1024;
pub const FIREHOSE_MAX_SEGMENT_FRAMES: usize = 100_000;
const FIREHOSE_MAX_STREAM_BYTES: usize = 256;
const FIREHOSE_MAX_SCOPE_BYTES: usize = 4 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentBytes(pub Vec<u8>);

impl SegmentBytes {
    pub fn encode(frames: &[Frame]) -> SegmentBytes {
        let mut out = Vec::new();
        out.extend_from_slice(&(frames.len() as u64).to_le_bytes());
        for f in frames {
            out.extend_from_slice(&f.seq.to_le_bytes());
            let body = f.payload.0.as_bytes();
            out.extend_from_slice(&(body.len() as u64).to_le_bytes());
            out.extend_from_slice(body);
        }
        SegmentBytes(out)
    }

    pub fn decode(bytes: &[u8]) -> Option<Vec<Frame>> {
        Self::decode_bounded(
            bytes,
            FIREHOSE_MAX_SEGMENT_BYTES,
            FIREHOSE_MAX_SEGMENT_FRAMES,
        )
    }

    pub fn decode_bounded(
        bytes: &[u8],
        maximum_bytes: usize,
        maximum_frames: usize,
    ) -> Option<Vec<Frame>> {
        if bytes.len() > maximum_bytes {
            return None;
        }
        let mut cur = 0usize;
        let count = usize::try_from(read_u64(bytes, &mut cur)?).ok()?;
        if count > maximum_frames {
            return None;
        }
        let mut frames = Vec::with_capacity(count.min(1 << 16));
        for _ in 0..count {
            let seq = read_u64(bytes, &mut cur)?;
            let len = read_u64(bytes, &mut cur)? as usize;
            let end = cur.checked_add(len)?;
            if end > bytes.len() {
                return None;
            }
            let body = std::str::from_utf8(&bytes[cur..end]).ok()?.to_string();
            cur = end;
            frames.push(Frame {
                seq,
                payload: FramePayload(body),
            });
        }
        if cur != bytes.len() {
            return None;
        }
        Some(frames)
    }
}

fn read_u64(bytes: &[u8], cur: &mut usize) -> Option<u64> {
    let end = cur.checked_add(8)?;
    if end > bytes.len() {
        return None;
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[*cur..end]);
    *cur = end;
    Some(u64::from_le_bytes(buf))
}

fn encoded_segment_len_bounded(
    frames: &[Frame],
    maximum_frames: usize,
    maximum_bytes: usize,
) -> Result<usize, ArchiveError> {
    if frames.len() > maximum_frames {
        return Err(ArchiveError::LimitExceeded("frame count"));
    }
    let encoded_bytes = frames.iter().try_fold(8usize, |total, frame| {
        total
            .checked_add(16)
            .and_then(|total| total.checked_add(frame.payload.0.len()))
    });
    match encoded_bytes {
        Some(encoded_bytes) if encoded_bytes <= maximum_bytes => Ok(encoded_bytes),
        _ => Err(ArchiveError::LimitExceeded("segment bytes")),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedSegment {
    pub tenant: TenantId,
    pub stream: String,
    pub scope: String,
    pub content_hash: ContentHash,
    pub first_seq: u64,
    pub last_seq: u64,
    pub frame_count: usize,
}

impl SealedSegment {
    pub fn covers(&self, seq: u64) -> bool {
        seq >= self.first_seq && seq <= self.last_seq
    }

    pub fn pointer_payload(&self) -> String {
        format!(
            "{}/{}/{}#{}-{}",
            self.stream,
            self.scope,
            self.content_hash.to_multihash_string(),
            self.first_seq,
            self.last_seq
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveError {
    EmptySegment,
    Blob(BlobError),
    CorruptSegment(ContentHash),
    LimitExceeded(&'static str),
}

impl core::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ArchiveError::EmptySegment => {
                write!(
                    f,
                    "firehose archive: refusing to seal an empty segment (no frames)"
                )
            }
            ArchiveError::Blob(e) => write!(f, "firehose archive: blob error: {e}"),
            ArchiveError::CorruptSegment(h) => write!(
                f,
                "firehose archive: sealed segment {} did not decode to a frame batch - corrupt, \
                 serve refused",
                h.to_multihash_string()
            ),
            ArchiveError::LimitExceeded(kind) => {
                write!(f, "firehose archive: {kind} limit exceeded")
            }
        }
    }
}

impl std::error::Error for ArchiveError {}

impl From<BlobError> for ArchiveError {
    fn from(e: BlobError) -> Self {
        ArchiveError::Blob(e)
    }
}

#[derive(Debug, Default)]
pub struct ArchiveTelemetry {
    sealed_segment_count: AtomicU64,
    unencrypted_segment_count: AtomicU64,
}

impl ArchiveTelemetry {
    pub fn sealed_segment_count(&self) -> u64 {
        self.sealed_segment_count.load(Ordering::SeqCst)
    }

    pub fn unencrypted_segment_count(&self) -> u64 {
        self.unencrypted_segment_count.load(Ordering::SeqCst)
    }

    pub fn segment_content_addressed(&self) -> bool {
        true
    }

    fn record_sealed(&self) {
        self.sealed_segment_count.fetch_add(1, Ordering::SeqCst);
    }

    pub fn record_unencrypted(&self) {
        self.unencrypted_segment_count
            .fetch_add(1, Ordering::SeqCst);
    }
}

pub struct FirehoseArchiver {
    tenant: TenantId,
    region: Region,
    store: FsBlobStore,
    segments: Mutex<Vec<SealedSegment>>,
    telemetry: ArchiveTelemetry,
}

impl FirehoseArchiver {
    pub fn with_tenant_dek(
        tenant: TenantId,
        region: Region,
        engine: std::sync::Arc<KmsEngine>,
    ) -> FirehoseArchiver {
        let wrap = DekContentWrap::new(
            engine,
            region.clone(),
            ErasureMethod::CryptoShred("tenant_dek".to_string()),
            None,
        );
        FirehoseArchiver {
            tenant,
            region,
            store: FsBlobStore::with_wrap(Box::new(wrap)),
            segments: Mutex::new(Vec::new()),
            telemetry: ArchiveTelemetry::default(),
        }
    }

    pub fn with_subject_dek(
        subject: crate::encryption::SubjectId,
        tenant: TenantId,
        region: Region,
        engine: std::sync::Arc<KmsEngine>,
    ) -> FirehoseArchiver {
        let wrap = DekContentWrap::new(
            engine,
            region.clone(),
            ErasureMethod::CryptoShred("subject_dek".to_string()),
            Some(subject),
        );
        FirehoseArchiver {
            tenant,
            region,
            store: FsBlobStore::with_wrap(Box::new(wrap)),
            segments: Mutex::new(Vec::new()),
            telemetry: ArchiveTelemetry::default(),
        }
    }

    pub fn seal(
        &self,
        stream: &str,
        scope: &str,
        frames: &[Frame],
    ) -> Result<SealedSegment, ArchiveError> {
        if frames.is_empty() {
            return Err(ArchiveError::EmptySegment);
        }
        if stream.len() > FIREHOSE_MAX_STREAM_BYTES {
            return Err(ArchiveError::LimitExceeded("stream"));
        }
        if scope.len() > FIREHOSE_MAX_SCOPE_BYTES {
            return Err(ArchiveError::LimitExceeded("scope"));
        }
        encoded_segment_len_bounded(
            frames,
            FIREHOSE_MAX_SEGMENT_FRAMES,
            FIREHOSE_MAX_SEGMENT_BYTES,
        )?;
        let bytes = SegmentBytes::encode(frames);
        let content_hash = self.store.put(&self.tenant, &bytes.0)?;
        let first_seq = frames.first().expect("non-empty checked above").seq;
        let last_seq = frames.last().expect("non-empty checked above").seq;
        let segment = SealedSegment {
            tenant: self.tenant.clone(),
            stream: stream.to_string(),
            scope: scope.to_string(),
            content_hash,
            first_seq,
            last_seq,
            frame_count: frames.len(),
        };
        self.segments
            .lock()
            .expect("archive mutex")
            .push(segment.clone());
        self.telemetry.record_sealed();
        Ok(segment)
    }

    pub fn seal_from_firehose(
        &self,
        firehose: &myelin_events::Firehose,
        stream: &str,
        scope: &myelin_events::FirehoseScope,
        lo: u64,
        hi: u64,
    ) -> Result<Option<SealedSegment>, ArchiveError> {
        let frames = firehose
            .tail_bounded(
                stream,
                scope,
                lo,
                hi,
                FIREHOSE_MAX_SEGMENT_FRAMES,
                FIREHOSE_MAX_SEGMENT_BYTES,
            )
            .map_err(|_| ArchiveError::LimitExceeded("firehose tail"))?;
        if frames.is_empty() {
            return Ok(None);
        }
        self.seal(stream, &scope.selector(), &frames).map(Some)
    }

    pub fn read_segment(&self, content_hash: &ContentHash) -> Result<Vec<Frame>, ArchiveError> {
        let metadata = self.store.head(&self.tenant, content_hash)?;
        if metadata.stored_len > FIREHOSE_MAX_STORED_SEGMENT_BYTES {
            return Err(ArchiveError::LimitExceeded("stored segment bytes"));
        }
        let bytes = self.store.get(&self.tenant, content_hash)?;
        SegmentBytes::decode_bounded(
            &bytes,
            FIREHOSE_MAX_SEGMENT_BYTES,
            FIREHOSE_MAX_SEGMENT_FRAMES,
        )
        .ok_or_else(|| ArchiveError::CorruptSegment(content_hash.clone()))
    }

    pub fn segment_covering(&self, seq: u64) -> Option<SealedSegment> {
        self.segments
            .lock()
            .expect("archive mutex")
            .iter()
            .find(|s| s.covers(seq))
            .cloned()
    }

    pub fn telemetry(&self) -> &ArchiveTelemetry {
        &self.telemetry
    }

    pub fn sealed_segment_count(&self) -> usize {
        self.segments.lock().expect("archive mutex").len()
    }

    pub fn residency_report(&self) -> StoreResidencyReport {
        StoreResidencyReport {
            tenant: self.tenant.clone(),
            store_class: ResidencyStoreClass::T3FirehoseArchive,
            region: self.region.clone(),
        }
    }

    #[doc(hidden)]
    pub fn corrupt_segment_for_drill(&self, content_hash: &ContentHash) -> bool {
        self.store.corrupt_for_drill(&self.tenant, content_hash)
    }
}

pub fn segment_pointer_draft(segment: &SealedSegment) -> FrameDraft {
    FrameDraft::new(segment.pointer_payload())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kms::{DekId, KekId, KeyClass};

    fn tenant() -> TenantId {
        TenantId("acme".to_string())
    }
    fn region() -> Region {
        Region("fr-par".to_string())
    }
    fn engine() -> std::sync::Arc<KmsEngine> {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant(), region()))
            .expect("seed the in-memory KEK");
        std::sync::Arc::new(kms)
    }
    fn archiver(engine: std::sync::Arc<KmsEngine>) -> FirehoseArchiver {
        FirehoseArchiver::with_tenant_dek(tenant(), region(), engine)
    }
    fn frames(seqs: &[u64]) -> Vec<Frame> {
        seqs.iter()
            .map(|&s| Frame::new(s, format!("op-{s}")))
            .collect()
    }

    #[test]
    fn segment_bytes_encode_decode_round_trips_exactly() {
        let fs = frames(&[3, 4, 5]);
        let bytes = SegmentBytes::encode(&fs);
        let back = SegmentBytes::decode(&bytes.0).expect("round-trip decode");
        assert_eq!(back, fs, "the exact (seq, payload) frame batch round-trips");
    }

    #[test]
    fn segment_encode_and_decode_enforce_count_and_byte_limits() {
        let fs = frames(&[3, 4]);
        let bytes = SegmentBytes::encode(&fs).0;

        assert_eq!(
            encoded_segment_len_bounded(&fs, 2, bytes.len()).expect("exact limits accepted"),
            bytes.len()
        );
        assert!(encoded_segment_len_bounded(&fs, 1, bytes.len()).is_err());
        assert!(encoded_segment_len_bounded(&fs, 2, bytes.len() - 1).is_err());
        assert_eq!(
            SegmentBytes::decode_bounded(&bytes, bytes.len(), 2).expect("exact decode limits"),
            fs
        );
        assert!(SegmentBytes::decode_bounded(&bytes, bytes.len() - 1, 2).is_none());
        assert!(SegmentBytes::decode_bounded(&bytes, bytes.len(), 1).is_none());
    }

    #[test]
    fn segment_bytes_encode_is_deterministic_for_a_stable_address() {
        let fs = frames(&[1, 2]);
        assert_eq!(SegmentBytes::encode(&fs), SegmentBytes::encode(&fs));
        assert_ne!(
            SegmentBytes::encode(&fs),
            SegmentBytes::encode(&frames(&[1, 3]))
        );
    }

    #[test]
    fn segment_bytes_decode_handles_a_frame_whose_last_read_consumes_exactly_the_tail() {
        let fs = vec![Frame::new(9, "")];
        let bytes = SegmentBytes::encode(&fs).0;
        let back = SegmentBytes::decode(&bytes).expect("an exactly-consumed tail is valid");
        assert_eq!(
            back, fs,
            "an empty-payload frame round-trips (the tail-read boundary)"
        );
    }

    #[test]
    fn segment_bytes_decode_rejects_truncation_and_trailing_garbage() {
        let bytes = SegmentBytes::encode(&frames(&[7])).0;
        assert!(
            SegmentBytes::decode(&bytes[..bytes.len() - 1]).is_none(),
            "truncated → None"
        );
        let mut extra = bytes.clone();
        extra.push(0xAB);
        assert!(
            SegmentBytes::decode(&extra).is_none(),
            "trailing garbage → None"
        );
        assert!(SegmentBytes::decode(&[]).is_none(), "no header → None");
    }

    #[test]
    fn seal_produces_a_content_addressed_dek_encrypted_segment() {
        let arch = archiver(engine());
        let fs = frames(&[1, 2, 3]);
        let seg = arch.seal("oplog", "board:42", &fs).expect("seal");

        assert_eq!(
            seg.content_hash,
            ContentHash::blake3(&SegmentBytes::encode(&fs).0)
        );
        assert!(seg
            .content_hash
            .to_multihash_string()
            .starts_with("blake3:"));
        assert!(arch.telemetry().segment_content_addressed());

        assert_eq!((seg.first_seq, seg.last_seq, seg.frame_count), (1, 3, 3));

        assert_eq!(arch.telemetry().unencrypted_segment_count(), 0);
        assert_eq!(
            arch.telemetry().sealed_segment_count(),
            1,
            "exactly one segment sealed"
        );
        assert_eq!(arch.sealed_segment_count(), 1, "the pointer count agrees");
        arch.seal("oplog", "board:42", &frames(&[4, 5]))
            .expect("second seal");
        assert_eq!(
            arch.telemetry().sealed_segment_count(),
            2,
            "the telemetry counts each seal"
        );
        assert_eq!(
            arch.sealed_segment_count(),
            2,
            "the pointer count counts each seal"
        );

        assert_eq!(arch.read_segment(&seg.content_hash).expect("read"), fs);
    }

    #[test]
    fn a_sealed_segment_rests_as_ciphertext_not_plaintext() {
        let arch = archiver(engine());
        let fs = vec![Frame::new(1, "SECRET-LOG-MARKER-payload")];
        let seg = arch.seal("oplog", "doc:x", &fs).expect("seal");

        let stored_len = arch
            .store
            .head(&tenant(), &seg.content_hash)
            .expect("head")
            .stored_len;
        let plaintext = SegmentBytes::encode(&fs).0;
        assert!(
            stored_len > plaintext.len(),
            "stored is the ciphertext envelope, not plaintext"
        );
        assert_eq!(arch.read_segment(&seg.content_hash).expect("read"), fs);
    }

    #[test]
    fn destroying_the_tenant_dek_crypto_shreds_the_segment() {
        let eng = engine();
        let arch = archiver(eng.clone());
        let seg = arch
            .seal("oplog", "channel:eng", &frames(&[1]))
            .expect("seal");
        assert!(
            arch.read_segment(&seg.content_hash).is_ok(),
            "reads before the shred"
        );

        assert!(eng
            .destroy_dek(&DekId::new(tenant(), KeyClass::Tenant))
            .unwrap());

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            arch.read_segment(&seg.content_hash)
        }));
        assert!(
            result.is_err(),
            "a crypto-shredded segment is unrecoverable (LOUD), never served"
        );
    }

    #[test]
    fn with_subject_dek_seals_under_the_subject_and_shreds_only_that_subject() {
        use crate::encryption::SubjectId;
        let eng = engine();
        let arch = FirehoseArchiver::with_subject_dek(
            SubjectId::new("u-alice"),
            tenant(),
            region(),
            eng.clone(),
        );
        let seg = arch.seal("ci-logs", "run:1", &frames(&[1])).expect("seal");
        assert_eq!(
            arch.read_segment(&seg.content_hash).expect("read"),
            frames(&[1])
        );

        eng.destroy_dek(&DekId::new(tenant(), KeyClass::Tenant))
            .unwrap();
        assert_eq!(
            arch.read_segment(&seg.content_hash)
                .expect("read after tenant-DEK destroy"),
            frames(&[1]),
            "the subject-keyed segment is not shredded by the tenant DEK destroy"
        );
        assert!(eng
            .destroy_dek(&DekId::new(tenant(), KeyClass::Subject("u-alice".into())))
            .unwrap());
        let after = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            arch.read_segment(&seg.content_hash)
        }));
        assert!(
            after.is_err(),
            "the subject's segment is crypto-shredded (unrecoverable)"
        );
    }

    #[test]
    fn seal_refuses_an_empty_segment() {
        let arch = archiver(engine());
        assert_eq!(
            arch.sealed_segment_count(),
            0,
            "a fresh archive has sealed nothing"
        );
        assert_eq!(
            arch.telemetry().sealed_segment_count(),
            0,
            "the telemetry starts at 0"
        );
        assert_eq!(
            arch.seal("oplog", "board:1", &[]).unwrap_err(),
            ArchiveError::EmptySegment
        );
        assert_eq!(
            arch.sealed_segment_count(),
            0,
            "a refused empty seal stores nothing"
        );
        assert_eq!(
            arch.telemetry().sealed_segment_count(),
            0,
            "a refused seal does not count"
        );
    }

    #[test]
    fn segment_covering_resolves_a_seq_to_its_segment() {
        let arch = archiver(engine());
        arch.seal("oplog", "doc:a", &frames(&[1, 2, 3]))
            .expect("seal");
        arch.seal("oplog", "doc:a", &frames(&[4, 5, 6]))
            .expect("seal");

        assert_eq!(arch.segment_covering(2).expect("covers 2").first_seq, 1);
        assert_eq!(arch.segment_covering(5).expect("covers 5").first_seq, 4);
        assert!(
            arch.segment_covering(99).is_none(),
            "no segment covers an out-of-range seq"
        );
    }

    #[test]
    fn covers_is_inclusive_on_both_ends() {
        let seg = SealedSegment {
            tenant: tenant(),
            stream: "s".into(),
            scope: "board:1".into(),
            content_hash: ContentHash::blake3(b"x"),
            first_seq: 3,
            last_seq: 5,
            frame_count: 3,
        };
        assert!(
            seg.covers(3) && seg.covers(4) && seg.covers(5),
            "inclusive both ends"
        );
        assert!(
            !seg.covers(2) && !seg.covers(6),
            "exclusive outside the range"
        );
    }

    #[test]
    fn pointer_payload_is_pii_free_and_names_the_segment() {
        let arch = archiver(engine());
        let seg = arch
            .seal("oplog", "board:42", &frames(&[1, 2]))
            .expect("seal");
        let payload = seg.pointer_payload();
        assert!(payload.contains(&seg.content_hash.to_multihash_string()));
        assert!(payload.contains("1-2"));
        assert!(
            !payload.contains("op-1"),
            "the pointer must NOT inline the frame body (PII-free)"
        );
        assert_eq!(segment_pointer_draft(&seg).payload.0, payload);
    }

    #[test]
    fn a_corrupt_segment_is_caught_loudly_on_read() {
        let arch = archiver(engine());
        let seg = arch.seal("oplog", "doc:x", &frames(&[1, 2])).expect("seal");
        assert!(
            arch.corrupt_segment_for_drill(&seg.content_hash),
            "segment present to corrupt"
        );
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            arch.read_segment(&seg.content_hash)
        }));
        match result {
            Err(_panic) => {}
            Ok(read) => {
                let err = read.expect_err("a corrupt segment must NOT serve");
                assert!(
                    matches!(err, ArchiveError::Blob(_) | ArchiveError::CorruptSegment(_)),
                    "corruption is a loud Blob/CorruptSegment error, got {err}"
                );
            }
        }
    }

    #[test]
    fn residency_report_is_the_t3_store_class_at_the_pinned_region() {
        let arch = archiver(engine());
        let report = arch.residency_report();
        assert_eq!(report.store_class, ResidencyStoreClass::T3FirehoseArchive);
        assert_eq!(report.region, region());
        assert_eq!(report.tenant, tenant());
    }

    #[test]
    fn telemetry_unencrypted_counter_is_a_real_detector() {
        let tel = ArchiveTelemetry::default();
        assert_eq!(tel.unencrypted_segment_count(), 0);
        tel.record_unencrypted();
        assert_eq!(
            tel.unencrypted_segment_count(),
            1,
            "the leak detector counts"
        );
        assert!(
            tel.segment_content_addressed(),
            "content-addressed by construction"
        );
    }

    #[test]
    fn archive_error_display_is_loud_and_specific() {
        assert!(ArchiveError::EmptySegment
            .to_string()
            .contains("empty segment"));
        assert!(ArchiveError::Blob(BlobError::UnknownAlgo("md5".into()))
            .to_string()
            .contains("blob error"));
        assert!(ArchiveError::CorruptSegment(ContentHash::blake3(b"x"))
            .to_string()
            .contains("corrupt"));
    }
}

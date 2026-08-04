use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_events::{Frame, FramePayload};
use myelin_tenancy::{Region, TenantId};

use crate::blob::ContentHash;
use crate::encryption::SubjectId;
use crate::firehose_archive::{
    ArchiveError, FirehoseArchiver, SealedSegment, FIREHOSE_MAX_SEGMENT_BYTES,
};
use crate::kms::KmsEngine;

const CI_LOG_MAX_SPANS_PER_STEP: usize = 10_000;
const CI_LOG_MAX_RESOLVED_STEP_BYTES: usize = 64 * 1024 * 1024;
const CI_LOG_MAX_BATCH_FRAMES: usize = 10_000;
const CI_LOG_MAX_JOB_ID_BYTES: usize = 1024;
const CI_LOG_MAX_CHUNK_BYTES: usize = 8 * 1024 * 1024;
const CI_LOG_MAX_FRAME_PAYLOAD_BYTES: usize = 2 * CI_LOG_MAX_CHUNK_BYTES + 2048;
use crate::residency::StoreResidencyReport;

pub const CI_LOG_STREAM: &str = "ci-logs";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiLogFrame {
    pub job_id: String,
    pub step_no: u32,
    pub bytes: Vec<u8>,
}

impl CiLogFrame {
    pub fn new(job_id: impl Into<String>, step_no: u32, bytes: impl Into<Vec<u8>>) -> CiLogFrame {
        CiLogFrame {
            job_id: job_id.into(),
            step_no,
            bytes: bytes.into(),
        }
    }

    pub fn to_payload(&self) -> Result<FramePayload, CiLogError> {
        let payload_len = self.encoded_payload_len_bounded()?;
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut payload = format!("{}\u{1}{}\u{1}", self.step_no, self.job_id);
        payload.reserve(payload_len.saturating_sub(payload.len()));
        for byte in &self.bytes {
            payload.push(HEX[(byte >> 4) as usize] as char);
            payload.push(HEX[(byte & 0x0f) as usize] as char);
        }
        Ok(FramePayload(payload))
    }

    pub fn from_payload(payload: &FramePayload) -> Option<CiLogFrame> {
        if payload.0.len() > CI_LOG_MAX_FRAME_PAYLOAD_BYTES {
            return None;
        }
        let mut parts = payload.0.splitn(3, '\u{1}');
        let step_no: u32 = parts.next()?.parse().ok()?;
        let job_id = parts.next()?.to_string();
        let hex = parts.next()?;
        if job_id.len() > CI_LOG_MAX_JOB_ID_BYTES
            || hex.len() % 2 != 0
            || hex.len() / 2 > CI_LOG_MAX_CHUNK_BYTES
        {
            return None;
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        let raw = hex.as_bytes();
        let mut i = 0;
        while i < raw.len() {
            let hi = hex_val(raw[i])?;
            let lo = hex_val(raw[i + 1])?;
            bytes.push(hi * 16 + lo);
            i += 2;
        }
        Some(CiLogFrame {
            job_id,
            step_no,
            bytes,
        })
    }

    fn encoded_payload_len_bounded(&self) -> Result<usize, CiLogError> {
        self.encoded_payload_len_with_limits(
            CI_LOG_MAX_JOB_ID_BYTES,
            CI_LOG_MAX_CHUNK_BYTES,
            CI_LOG_MAX_FRAME_PAYLOAD_BYTES,
        )
    }

    fn encoded_payload_len_with_limits(
        &self,
        maximum_job_id_bytes: usize,
        maximum_chunk_bytes: usize,
        maximum_payload_bytes: usize,
    ) -> Result<usize, CiLogError> {
        if self.job_id.len() > maximum_job_id_bytes {
            return Err(CiLogError::LimitExceeded("CI log job id bytes"));
        }
        if self.bytes.len() > maximum_chunk_bytes {
            return Err(CiLogError::LimitExceeded("CI log chunk bytes"));
        }
        let payload_len = self
            .bytes
            .len()
            .checked_mul(2)
            .and_then(|len| len.checked_add(self.job_id.len()))
            .and_then(|len| len.checked_add(self.step_no.to_string().len()))
            .and_then(|len| len.checked_add(2))
            .ok_or(CiLogError::LimitExceeded("CI log frame payload bytes"))?;
        if payload_len > maximum_payload_bytes {
            return Err(CiLogError::LimitExceeded("CI log frame payload bytes"));
        }
        Ok(payload_len)
    }
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SegmentKeying {
    Tenant,
    Subject(SubjectId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepSpan {
    pub segment: ContentHash,
    pub offset: u64,
    pub len: u64,
    pub frame_seq: u64,
    pub keying: SegmentKeying,
}

impl StepSpan {
    pub fn byte_range(&self) -> Result<(u64, u64), CiLogError> {
        let end = self
            .offset
            .checked_add(self.len)
            .ok_or(CiLogError::LimitExceeded("step span byte range"))?;
        Ok((self.offset, end))
    }
}

#[derive(Debug, Default)]
pub struct CiLogIndex {
    by_step: BTreeMap<(String, u32), Vec<StepSpan>>,
}

impl CiLogIndex {
    pub fn new() -> CiLogIndex {
        CiLogIndex::default()
    }

    fn append(&mut self, job_id: &str, step_no: u32, span: StepSpan) -> bool {
        let spans = self
            .by_step
            .entry((job_id.to_string(), step_no))
            .or_default();
        if spans.iter().any(|s| s.frame_seq == span.frame_seq) {
            return false;
        }
        spans.push(span);
        spans.sort_by_key(|s| s.frame_seq);
        true
    }

    pub fn spans(&self, job_id: &str, step_no: u32) -> Option<&[StepSpan]> {
        self.by_step
            .get(&(job_id.to_string(), step_no))
            .map(|v| v.as_slice())
    }

    pub fn step_log_len(&self, job_id: &str, step_no: u32) -> u64 {
        self.checked_step_log_len(job_id, step_no)
            .unwrap_or(u64::MAX)
    }

    fn checked_step_log_len(&self, job_id: &str, step_no: u32) -> Option<u64> {
        self.by_step
            .get(&(job_id.to_string(), step_no))
            .map(|spans| {
                spans
                    .iter()
                    .try_fold(0u64, |total, span| total.checked_add(span.len))
            })
            .unwrap_or(Some(0))
    }

    pub fn step_count(&self) -> usize {
        self.by_step.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiLogError {
    UnknownStep { job_id: String, step_no: u32 },
    MalformedAnchor(String),
    Archive(ArchiveError),
    SpanOutOfBounds {
        segment: ContentHash,
        offset: u64,
        len: u64,
    },
    LimitExceeded(&'static str),
}

impl core::fmt::Display for CiLogError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CiLogError::UnknownStep { job_id, step_no } => write!(
                f,
                "ci log index: no log for (job={job_id}, step={step_no}) - #step-{step_no} unresolvable"
            ),
            CiLogError::MalformedAnchor(a) => {
                write!(f, "ci log index: malformed #step-<n> anchor: {a}")
            }
            CiLogError::Archive(e) => write!(f, "ci log index: segment archive error: {e}"),
            CiLogError::SpanOutOfBounds {
                segment,
                offset,
                len,
            } => write!(
                f,
                "ci log index: span [{offset}, {}) out of bounds in segment {} - corrupt index, serve refused",
                offset.saturating_add(*len),
                segment.to_multihash_string()
            ),
            CiLogError::LimitExceeded(kind) => {
                write!(f, "ci log index: {kind} limit exceeded")
            }
        }
    }
}

impl std::error::Error for CiLogError {}

impl From<ArchiveError> for CiLogError {
    fn from(e: ArchiveError) -> Self {
        CiLogError::Archive(e)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepAnchor {
    pub run_id: String,
    pub step_no: u32,
}

impl StepAnchor {
    pub fn parse(anchor: &str) -> Result<StepAnchor, CiLogError> {
        let malformed = || CiLogError::MalformedAnchor(anchor.to_string());
        let (path, frag) = anchor.split_once('#').ok_or_else(malformed)?;
        let step_no: u32 = frag
            .strip_prefix("step-")
            .ok_or_else(malformed)?
            .parse()
            .map_err(|_| malformed())?;
        let run_id = path
            .split("ci/run/")
            .nth(1)
            .filter(|r| !r.is_empty())
            .ok_or_else(malformed)?
            .trim_end_matches('/')
            .to_string();
        if run_id.is_empty() {
            return Err(malformed());
        }
        Ok(StepAnchor { run_id, step_no })
    }
}

pub struct CiLogTier {
    run_id: String,
    tenant: TenantId,
    region: Region,
    engine: std::sync::Arc<KmsEngine>,
    archiver: FirehoseArchiver,
    subject_archivers: Mutex<BTreeMap<String, std::sync::Arc<FirehoseArchiver>>>,
    index: Mutex<CiLogIndex>,
}

impl CiLogTier {
    pub fn with_tenant_dek(
        run_id: impl Into<String>,
        tenant: TenantId,
        region: Region,
        engine: std::sync::Arc<KmsEngine>,
    ) -> CiLogTier {
        CiLogTier {
            run_id: run_id.into(),
            tenant: tenant.clone(),
            region: region.clone(),
            engine: engine.clone(),
            archiver: FirehoseArchiver::with_tenant_dek(tenant, region, engine),
            subject_archivers: Mutex::new(BTreeMap::new()),
            index: Mutex::new(CiLogIndex::new()),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    fn subject_archiver(&self, subject: &SubjectId) -> std::sync::Arc<FirehoseArchiver> {
        let mut map = self
            .subject_archivers
            .lock()
            .expect("ci log subject-archiver mutex");
        map.entry(subject.0.clone())
            .or_insert_with(|| {
                std::sync::Arc::new(FirehoseArchiver::with_subject_dek(
                    subject.clone(),
                    self.tenant.clone(),
                    self.region.clone(),
                    self.engine.clone(),
                ))
            })
            .clone()
    }

    pub fn seal_ci_batch(&self, frames: &[(u64, CiLogFrame)]) -> Result<SealedSegment, CiLogError> {
        self.seal_through(&self.archiver, SegmentKeying::Tenant, frames)
    }

    pub fn seal_ci_batch_for_subject(
        &self,
        subject: &SubjectId,
        frames: &[(u64, CiLogFrame)],
    ) -> Result<SealedSegment, CiLogError> {
        let archiver = self.subject_archiver(subject);
        self.seal_through(&archiver, SegmentKeying::Subject(subject.clone()), frames)
    }

    fn seal_through(
        &self,
        archiver: &FirehoseArchiver,
        keying: SegmentKeying,
        frames: &[(u64, CiLogFrame)],
    ) -> Result<SealedSegment, CiLogError> {
        if frames.len() > CI_LOG_MAX_BATCH_FRAMES {
            return Err(CiLogError::LimitExceeded("CI log batch frame count"));
        }
        let mut encoded_segment_bytes = 8usize;
        for (_, frame) in frames {
            let frame_payload_bytes = frame.encoded_payload_len_bounded()?;
            encoded_segment_bytes = encoded_segment_bytes
                .checked_add(16)
                .and_then(|total| total.checked_add(frame_payload_bytes))
                .ok_or(CiLogError::LimitExceeded("CI log segment bytes"))?;
            if encoded_segment_bytes > FIREHOSE_MAX_SEGMENT_BYTES {
                return Err(CiLogError::LimitExceeded("CI log segment bytes"));
            }
        }
        let scope_selector = format!("run:{}", self.run_id);
        let mut transport = Vec::with_capacity(frames.len());
        for (seq, frame) in frames {
            transport.push(Frame {
                seq: *seq,
                payload: frame.to_payload()?,
            });
        }
        let segment = archiver.seal(CI_LOG_STREAM, &scope_selector, &transport)?;

        let mut index = self.index.lock().expect("ci log index mutex");
        for (seq, clf) in frames {
            let offset = index
                .checked_step_log_len(&clf.job_id, clf.step_no)
                .ok_or(CiLogError::LimitExceeded("step log offset"))?;
            let span = StepSpan {
                segment: segment.content_hash.clone(),
                offset,
                len: clf.bytes.len() as u64,
                frame_seq: *seq,
                keying: keying.clone(),
            };
            index.append(&clf.job_id, clf.step_no, span);
        }
        Ok(segment)
    }

    pub fn resolve_step(&self, job_id: &str, step_no: u32) -> Result<Vec<u8>, CiLogError> {
        self.resolve_step_bounded(
            job_id,
            step_no,
            CI_LOG_MAX_SPANS_PER_STEP,
            CI_LOG_MAX_RESOLVED_STEP_BYTES,
        )
    }

    fn resolve_step_bounded(
        &self,
        job_id: &str,
        step_no: u32,
        maximum_spans: usize,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, CiLogError> {
        let (spans, resolved_bytes): (Vec<StepSpan>, usize) = {
            let index = self.index.lock().expect("ci log index mutex");
            match index.spans(job_id, step_no) {
                Some(spans) => {
                    if spans.len() > maximum_spans {
                        return Err(CiLogError::LimitExceeded("step span count"));
                    }
                    let resolved_bytes = spans.iter().try_fold(0usize, |total, span| {
                        usize::try_from(span.len)
                            .ok()
                            .and_then(|len| total.checked_add(len))
                    });
                    match resolved_bytes {
                        Some(resolved_bytes) if resolved_bytes <= maximum_bytes => {
                            (spans.to_vec(), resolved_bytes)
                        }
                        _ => return Err(CiLogError::LimitExceeded("resolved step bytes")),
                    }
                }
                None => {
                    return Err(CiLogError::UnknownStep {
                        job_id: job_id.to_string(),
                        step_no,
                    })
                }
            }
        };

        let mut out = Vec::with_capacity(resolved_bytes);
        for span in &spans {
            let subject_archiver = match &span.keying {
                SegmentKeying::Tenant => None,
                SegmentKeying::Subject(s) => Some(self.subject_archiver(s)),
            };
            let archiver: &FirehoseArchiver = subject_archiver.as_deref().unwrap_or(&self.archiver);
            let frames = archiver.read_segment(&span.segment)?;
            let chunk = frames
                .iter()
                .find(|f| f.seq == span.frame_seq)
                .and_then(|f| CiLogFrame::from_payload(&f.payload))
                .filter(|clf| clf.job_id == job_id && clf.step_no == step_no)
                .ok_or_else(|| CiLogError::SpanOutOfBounds {
                    segment: span.segment.clone(),
                    offset: span.offset,
                    len: span.len,
                })?;
            if chunk.bytes.len() as u64 != span.len {
                return Err(CiLogError::SpanOutOfBounds {
                    segment: span.segment.clone(),
                    offset: span.offset,
                    len: span.len,
                });
            }
            out.extend_from_slice(&chunk.bytes);
        }
        Ok(out)
    }

    pub fn resolve_step_anchor(&self, anchor: &str) -> Result<Vec<u8>, CiLogError> {
        let parsed = StepAnchor::parse(anchor)?;
        if parsed.run_id != self.run_id {
            return Err(CiLogError::UnknownStep {
                job_id: parsed.run_id,
                step_no: parsed.step_no,
            });
        }
        self.resolve_step(&self.run_id, parsed.step_no)
    }

    pub fn indexed_step_count(&self) -> usize {
        self.index.lock().expect("ci log index mutex").step_count()
    }

    pub fn step_log_len(&self, job_id: &str, step_no: u32) -> u64 {
        self.index
            .lock()
            .expect("ci log index mutex")
            .step_log_len(job_id, step_no)
    }

    pub fn archiver(&self) -> &FirehoseArchiver {
        &self.archiver
    }

    pub fn step_keying(&self, job_id: &str, step_no: u32) -> Option<Vec<SegmentKeying>> {
        self.index
            .lock()
            .expect("ci log index mutex")
            .spans(job_id, step_no)
            .map(|spans| spans.iter().map(|s| s.keying.clone()).collect())
    }

    pub fn subject_keyed_count(&self) -> usize {
        self.subject_archivers
            .lock()
            .expect("ci log subject-archiver mutex")
            .len()
    }

    #[doc(hidden)]
    pub fn inject_desynced_span_for_drill(
        &self,
        job_id: &str,
        step_no: u32,
        segment: ContentHash,
        wrong_frame_seq: u64,
        claimed_len: u64,
    ) {
        let mut index = self.index.lock().expect("ci log index mutex");
        index.append(
            job_id,
            step_no,
            StepSpan {
                segment,
                offset: 0,
                len: claimed_len,
                frame_seq: wrong_frame_seq,
                keying: SegmentKeying::Tenant,
            },
        );
    }

    pub fn residency_report(&self) -> StoreResidencyReport {
        self.archiver.residency_report()
    }
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
    fn engine() -> std::sync::Arc<crate::kms::KmsEngine> {
        let kms = crate::kms::KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant(), region()));
        std::sync::Arc::new(kms)
    }
    fn tier(run: &str) -> CiLogTier {
        CiLogTier::with_tenant_dek(run, tenant(), region(), engine())
    }

    #[test]
    fn ci_log_frame_payload_round_trips_exactly_including_binary() {
        let clf = CiLogFrame::new("build", 3, vec![0x00, 0xff, b'l', b'o', b'g', 0x01]);
        let payload = clf.to_payload().expect("small payload");
        let back = CiLogFrame::from_payload(&payload).expect("round-trip");
        assert_eq!(
            back, clf,
            "the exact (job, step, bytes) round-trips, binary included"
        );
    }

    #[test]
    fn ci_log_frame_payload_is_deterministic() {
        let clf = CiLogFrame::new("test", 2, b"hello".to_vec());
        assert_eq!(clf.to_payload().unwrap(), clf.to_payload().unwrap());
        assert_ne!(
            clf.to_payload().unwrap(),
            CiLogFrame::new("test", 3, b"hello".to_vec())
                .to_payload()
                .unwrap()
        );
        assert_ne!(
            clf.to_payload().unwrap(),
            CiLogFrame::new("lint", 2, b"hello".to_vec())
                .to_payload()
                .unwrap()
        );
        assert_ne!(
            clf.to_payload().unwrap(),
            CiLogFrame::new("test", 2, b"hellp".to_vec())
                .to_payload()
                .unwrap()
        );
    }

    #[test]
    fn ci_log_frame_encoding_enforces_every_materialization_limit() {
        let frame = CiLogFrame::new("job", 1, b"ok".to_vec());
        assert_eq!(
            frame
                .encoded_payload_len_with_limits(3, 2, 10)
                .expect("exact limits accepted"),
            10
        );
        assert!(frame
            .encoded_payload_len_with_limits(2, 2, 10)
            .is_err());
        assert!(frame
            .encoded_payload_len_with_limits(3, 1, 10)
            .is_err());
        assert!(frame
            .encoded_payload_len_with_limits(3, 2, 9)
            .is_err());
    }

    #[test]
    fn ci_log_frame_from_payload_rejects_malformation_loudly() {
        assert!(CiLogFrame::from_payload(&FramePayload("3".into())).is_none());
        assert!(CiLogFrame::from_payload(&FramePayload("3\u{1}job".into())).is_none());
        assert!(CiLogFrame::from_payload(&FramePayload("x\u{1}job\u{1}6c6f67".into())).is_none());
        assert!(CiLogFrame::from_payload(&FramePayload("3\u{1}job\u{1}abc".into())).is_none());
        assert!(CiLogFrame::from_payload(&FramePayload("3\u{1}job\u{1}zz".into())).is_none());
        let ok = CiLogFrame::from_payload(&FramePayload("3\u{1}job\u{1}".into()))
            .expect("empty chunk ok");
        assert_eq!(ok, CiLogFrame::new("job", 3, Vec::<u8>::new()));
    }

    #[test]
    fn ci_log_frame_hex_decode_is_byte_exact_for_every_nibble_path() {
        let one_b = CiLogFrame::from_payload(&FramePayload("0\u{1}j\u{1}1b".into())).expect("1b");
        assert_eq!(one_b.bytes, vec![0x1b]);

        let ff = CiLogFrame::from_payload(&FramePayload("0\u{1}j\u{1}FF".into())).expect("FF");
        assert_eq!(ff.bytes, vec![0xff]);
        let af = CiLogFrame::from_payload(&FramePayload("0\u{1}j\u{1}Af".into())).expect("Af");
        assert_eq!(af.bytes, vec![0xaf]);

        let a0 = CiLogFrame::from_payload(&FramePayload("0\u{1}j\u{1}a0".into())).expect("a0");
        assert_eq!(a0.bytes, vec![0xa0]);

        let all: Vec<u8> = (0..=255u16).map(|b| b as u8).collect();
        let rt = CiLogFrame::from_payload(
            &CiLogFrame::new("j", 1, all.clone())
                .to_payload()
                .expect("small payload"),
        )
        .expect("all-bytes round-trip");
        assert_eq!(rt.bytes, all);
    }

    #[test]
    fn seal_then_resolve_step_returns_exactly_that_steps_bytes() {
        let t = tier("run-1");
        let frames = vec![
            (1, CiLogFrame::new("run-1", 1, b"checkout ok\n".to_vec())),
            (
                2,
                CiLogFrame::new("run-1", 2, b"build started\nbuild done\n".to_vec()),
            ),
            (
                3,
                CiLogFrame::new("run-1", 3, b"FAIL: assertion at line 42\n".to_vec()),
            ),
        ];
        let seg = t.seal_ci_batch(&frames).expect("seal ci batch");

        assert_eq!(t.run_id(), "run-1");

        assert!(seg
            .content_hash
            .to_multihash_string()
            .starts_with("blake3:"));
        assert_eq!(t.archiver().telemetry().unencrypted_segment_count(), 0);
        assert!(t.archiver().telemetry().segment_content_addressed());
        assert_eq!(t.indexed_step_count(), 3, "three (job, step) keys indexed");

        assert_eq!(t.resolve_step("run-1", 1).unwrap(), b"checkout ok\n");
        assert_eq!(
            t.resolve_step("run-1", 2).unwrap(),
            b"build started\nbuild done\n"
        );
        assert_eq!(
            t.resolve_step("run-1", 3).unwrap(),
            b"FAIL: assertion at line 42\n"
        );
    }

    #[test]
    fn step_anchor_resolves_to_the_exact_failing_step_bytes() {
        let t = tier("run-7");
        let frames = vec![
            (1, CiLogFrame::new("run-7", 1, b"ok\n".to_vec())),
            (2, CiLogFrame::new("run-7", 2, b"FAILURE HERE\n".to_vec())),
        ];
        t.seal_ci_batch(&frames).expect("seal");

        let bytes = t
            .resolve_step_anchor("myelin://acme/ci/run/run-7#step-2")
            .expect("resolve the #step-2 jump-to-failure");
        assert_eq!(
            bytes, b"FAILURE HERE\n",
            "the anchor resolves to step 2's EXACT bytes"
        );
        assert_eq!(t.step_log_len("run-7", 2), bytes.len() as u64);
    }

    #[test]
    fn a_step_spanning_multiple_segments_concatenates_in_order() {
        let t = tier("run-9");
        t.seal_ci_batch(&[(1, CiLogFrame::new("run-9", 2, b"part-A ".to_vec()))])
            .expect("seal batch 1");
        t.seal_ci_batch(&[(2, CiLogFrame::new("run-9", 2, b"part-B".to_vec()))])
            .expect("seal batch 2");

        assert_eq!(t.resolve_step("run-9", 2).unwrap(), b"part-A part-B");
        assert_eq!(t.step_log_len("run-9", 2), 13);
        let index = t.index.lock().unwrap();
        let spans = index.spans("run-9", 2).unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].offset, spans[0].len), (0, 7));
        assert_eq!((spans[1].offset, spans[1].len), (7, 6));
        assert_ne!(
            spans[0].segment, spans[1].segment,
            "two distinct sealed segments"
        );
        drop(index);
        assert_eq!(
            t.resolve_step_bounded("run-9", 2, 2, 13)
                .expect("exact limits accepted"),
            b"part-A part-B"
        );
        assert_eq!(
            t.resolve_step_bounded("run-9", 2, 1, 13),
            Err(CiLogError::LimitExceeded("step span count"))
        );
        assert_eq!(
            t.resolve_step_bounded("run-9", 2, 2, 12),
            Err(CiLogError::LimitExceeded("resolved step bytes"))
        );
    }

    #[test]
    fn interleaved_steps_in_one_batch_index_independently() {
        let t = tier("run-x");
        let frames = vec![
            (1, CiLogFrame::new("run-x", 1, b"a1".to_vec())),
            (2, CiLogFrame::new("run-x", 2, b"b1".to_vec())),
            (3, CiLogFrame::new("run-x", 1, b"a2".to_vec())),
            (4, CiLogFrame::new("run-x", 2, b"b2".to_vec())),
        ];
        t.seal_ci_batch(&frames).expect("seal");
        assert_eq!(t.resolve_step("run-x", 1).unwrap(), b"a1a2");
        assert_eq!(t.resolve_step("run-x", 2).unwrap(), b"b1b2");
    }

    #[test]
    fn resolve_unknown_step_is_a_loud_miss() {
        let t = tier("run-1");
        t.seal_ci_batch(&[(1, CiLogFrame::new("run-1", 1, b"ok".to_vec()))])
            .expect("seal");
        assert_eq!(
            t.resolve_step("run-1", 99).unwrap_err(),
            CiLogError::UnknownStep {
                job_id: "run-1".into(),
                step_no: 99
            }
        );
        assert!(matches!(
            t.resolve_step_anchor("myelin://acme/ci/run/run-1#step-99"),
            Err(CiLogError::UnknownStep { .. })
        ));
    }

    #[test]
    fn a_desynced_span_pointing_at_the_wrong_step_is_a_loud_refusal_not_a_wrong_serve() {
        let t = tier("run-1");
        let seg = t
            .seal_ci_batch(&[(5, CiLogFrame::new("run-1", 1, b"STEP-ONE-BYTES".to_vec()))])
            .expect("seal");
        t.inject_desynced_span_for_drill("run-1", 2, seg.content_hash.clone(), 5, 14);

        let err = t
            .resolve_step("run-1", 2)
            .expect_err("a desync must refuse, never wrong-serve");
        assert!(
            matches!(err, CiLogError::SpanOutOfBounds { .. }),
            "a wrong-step desync is a LOUD SpanOutOfBounds, never step 1's bytes for a step-2 request, got {err}"
        );
        assert_eq!(t.resolve_step("run-1", 1).unwrap(), b"STEP-ONE-BYTES");
    }

    #[test]
    fn cross_run_anchor_does_not_resolve() {
        let t = tier("run-1");
        t.seal_ci_batch(&[(1, CiLogFrame::new("run-1", 1, b"ok".to_vec()))])
            .expect("seal");
        assert!(matches!(
            t.resolve_step_anchor("myelin://acme/ci/run/run-2#step-1"),
            Err(CiLogError::UnknownStep { .. })
        ));
    }

    #[test]
    fn step_anchor_parses_the_x1_shape() {
        let a = StepAnchor::parse("myelin://acme/ci/run/run-42#step-3").expect("parse");
        assert_eq!(
            a,
            StepAnchor {
                run_id: "run-42".into(),
                step_no: 3
            }
        );
        let b = StepAnchor::parse("acme/ci/run/run-42#step-3").expect("parse bare");
        assert_eq!(b, a);
    }

    #[test]
    fn step_anchor_rejects_malformation_loudly() {
        assert!(matches!(
            StepAnchor::parse("myelin://acme/ci/run/run-1"),
            Err(CiLogError::MalformedAnchor(_))
        ));
        assert!(matches!(
            StepAnchor::parse("myelin://acme/ci/run/run-1#frag-1"),
            Err(CiLogError::MalformedAnchor(_))
        ));
        assert!(matches!(
            StepAnchor::parse("myelin://acme/ci/run/run-1#step-x"),
            Err(CiLogError::MalformedAnchor(_))
        ));
        assert!(matches!(
            StepAnchor::parse("myelin://acme/issues/42#step-1"),
            Err(CiLogError::MalformedAnchor(_))
        ));
        assert!(matches!(
            StepAnchor::parse("myelin://acme/ci/run/#step-1"),
            Err(CiLogError::MalformedAnchor(_))
        ));
    }

    #[test]
    fn destroying_the_tenant_dek_crypto_shreds_the_ci_log_step_resolution() {
        let eng = engine();
        let t = CiLogTier::with_tenant_dek("run-1", tenant(), region(), eng.clone());
        t.seal_ci_batch(&[(
            1,
            CiLogFrame::new("run-1", 1, b"inline-PII-step-log".to_vec()),
        )])
        .expect("seal");
        assert!(
            t.resolve_step("run-1", 1).is_ok(),
            "resolves before the shred"
        );

        assert!(eng.destroy_dek(&DekId::new(tenant(), KeyClass::Tenant)));
        let res =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.resolve_step("run-1", 1)));
        assert!(
            res.is_err(),
            "a crypto-shredded CI log step is unrecoverable (LOUD), never served"
        );
    }

    #[test]
    fn an_isolable_pii_ci_log_segment_keys_under_the_subjects_dek() {
        let t = tier("run-1");
        let subject = SubjectId::new("u-alice");
        t.seal_ci_batch_for_subject(
            &subject,
            &[(
                1,
                CiLogFrame::new("run-1", 1, b"alice@example.test ran the build\n".to_vec()),
            )],
        )
        .expect("seal an isolable-PII segment under the subject DEK");

        assert_eq!(
            t.step_keying("run-1", 1).unwrap(),
            vec![SegmentKeying::Subject(subject.clone())],
            "an isolable-PII step is keyed under the subject's DEK (C1)"
        );
        assert_eq!(t.subject_keyed_count(), 1, "one subject's DEK is in use");
        assert_eq!(
            t.resolve_step("run-1", 1).unwrap(),
            b"alice@example.test ran the build\n"
        );
    }

    #[test]
    fn a_non_isolable_segment_falls_back_to_the_per_tenant_dek_the_documented_residual() {
        let t = tier("run-1");
        t.seal_ci_batch(&[(
            1,
            CiLogFrame::new(
                "run-1",
                1,
                b"alice & bob co-edited; interleaved log\n".to_vec(),
            ),
        )])
        .expect("seal a non-isolable segment under the tenant DEK");
        assert_eq!(
            t.step_keying("run-1", 1).unwrap(),
            vec![SegmentKeying::Tenant],
            "a non-isolable segment falls back to the per-tenant DEK (the residual)"
        );
        assert_eq!(
            t.subject_keyed_count(),
            0,
            "no subject DEK minted for the fallback"
        );
        assert_eq!(
            t.resolve_step("run-1", 1).unwrap(),
            b"alice & bob co-edited; interleaved log\n"
        );
    }

    #[test]
    fn erasing_a_subject_crypto_shreds_exactly_their_isolable_ci_log_without_touching_the_tenants()
    {
        let eng = engine();
        let t = CiLogTier::with_tenant_dek("run-1", tenant(), region(), eng.clone());
        let alice = SubjectId::new("u-alice");
        let bob = SubjectId::new("u-bob");

        t.seal_ci_batch_for_subject(
            &alice,
            &[(1, CiLogFrame::new("run-1", 1, b"ALICE-PII".to_vec()))],
        )
        .expect("seal alice");
        t.seal_ci_batch_for_subject(
            &bob,
            &[(2, CiLogFrame::new("run-1", 2, b"BOB-PII".to_vec()))],
        )
        .expect("seal bob");
        t.seal_ci_batch(&[(3, CiLogFrame::new("run-1", 3, b"INTERLEAVED".to_vec()))])
            .expect("seal interleaved");

        assert_eq!(t.resolve_step("run-1", 1).unwrap(), b"ALICE-PII");
        assert_eq!(t.resolve_step("run-1", 2).unwrap(), b"BOB-PII");
        assert_eq!(t.resolve_step("run-1", 3).unwrap(), b"INTERLEAVED");

        assert!(eng.destroy_dek(&DekId::new(tenant(), KeyClass::Subject(alice.0.clone()))));

        let alice_after =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.resolve_step("run-1", 1)));
        assert!(
            alice_after.is_err(),
            "alice's isolable CI log step is crypto-shredded (unrecoverable, LOUD)"
        );
        assert_eq!(
            t.resolve_step("run-1", 2).unwrap(),
            b"BOB-PII",
            "bob's log is untouched by alice's erasure"
        );
        assert_eq!(
            t.resolve_step("run-1", 3).unwrap(),
            b"INTERLEAVED",
            "the tenant-fallback log is untouched by alice's erasure"
        );
    }

    #[test]
    fn destroying_the_tenant_dek_does_not_shred_a_subject_keyed_ci_log() {
        let eng = engine();
        let t = CiLogTier::with_tenant_dek("run-1", tenant(), region(), eng.clone());
        let alice = SubjectId::new("u-alice");
        t.seal_ci_batch_for_subject(
            &alice,
            &[(1, CiLogFrame::new("run-1", 1, b"ALICE-PII".to_vec()))],
        )
        .expect("seal alice");
        t.seal_ci_batch(&[(2, CiLogFrame::new("run-1", 2, b"INTERLEAVED".to_vec()))])
            .expect("seal interleaved");

        assert!(eng.destroy_dek(&DekId::new(tenant(), KeyClass::Tenant)));
        assert_eq!(
            t.resolve_step("run-1", 1).unwrap(),
            b"ALICE-PII",
            "a subject-keyed CI log is not shredded by the tenant-DEK destroy (it keys per-subject)"
        );
        let tenant_after =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.resolve_step("run-1", 2)));
        assert!(
            tenant_after.is_err(),
            "the tenant-fallback step IS shredded by the tenant-DEK destroy"
        );
    }

    #[test]
    fn step_keying_is_none_for_an_unknown_step() {
        let t = tier("run-1");
        assert!(t.step_keying("run-1", 99).is_none());
    }

    #[test]
    fn seal_empty_subject_batch_is_refused() {
        let t = tier("run-1");
        assert!(matches!(
            t.seal_ci_batch_for_subject(&SubjectId::new("u-x"), &[]),
            Err(CiLogError::Archive(ArchiveError::EmptySegment))
        ));
        assert_eq!(
            t.subject_keyed_count(),
            1,
            "the archiver is minted but seals nothing"
        );
        assert_eq!(t.indexed_step_count(), 0, "a refused seal indexes nothing");
    }

    #[test]
    fn re_sealing_a_frame_seq_is_idempotent_in_the_index() {
        let t = tier("run-1");
        t.seal_ci_batch(&[(1, CiLogFrame::new("run-1", 1, b"line".to_vec()))])
            .expect("seal 1");
        t.seal_ci_batch(&[(1, CiLogFrame::new("run-1", 1, b"line".to_vec()))])
            .expect("re-seal 1");
        assert_eq!(
            t.step_log_len("run-1", 1),
            4,
            "the duplicate seq is absorbed (idempotent)"
        );
        assert_eq!(t.resolve_step("run-1", 1).unwrap(), b"line");
    }

    #[test]
    fn seal_empty_ci_batch_is_refused() {
        let t = tier("run-1");
        assert!(matches!(
            t.seal_ci_batch(&[]),
            Err(CiLogError::Archive(ArchiveError::EmptySegment))
        ));
        assert_eq!(t.indexed_step_count(), 0, "a refused seal indexes nothing");
    }

    #[test]
    fn step_span_byte_range_and_error_display() {
        let span = StepSpan {
            segment: ContentHash::blake3(b"x"),
            offset: 10,
            len: 5,
            frame_seq: 1,
            keying: SegmentKeying::Tenant,
        };
        assert_eq!(span.byte_range().unwrap(), (10, 15));
        assert_eq!(
            StepSpan {
                offset: u64::MAX,
                len: 1,
                ..span.clone()
            }
            .byte_range(),
            Err(CiLogError::LimitExceeded("step span byte range"))
        );
        let mut index = CiLogIndex::new();
        index.append(
            "job",
            1,
            StepSpan {
                offset: 0,
                len: u64::MAX,
                ..span.clone()
            },
        );
        index.append(
            "job",
            1,
            StepSpan {
                offset: u64::MAX,
                len: 1,
                frame_seq: 2,
                ..span.clone()
            },
        );
        assert_eq!(index.checked_step_log_len("job", 1), None);
        assert_eq!(index.step_log_len("job", 1), u64::MAX);
        assert!(CiLogError::UnknownStep {
            job_id: "j".into(),
            step_no: 4
        }
        .to_string()
        .contains("#step-4"));
        assert!(CiLogError::MalformedAnchor("bad".into())
            .to_string()
            .contains("malformed"));
        assert!(CiLogError::SpanOutOfBounds {
            segment: ContentHash::blake3(b"y"),
            offset: 0,
            len: 2
        }
        .to_string()
        .contains("out of bounds"));
    }

    #[test]
    fn residency_report_is_the_t3_firehose_archive_class() {
        let t = tier("run-1");
        let report = t.residency_report();
        assert_eq!(
            report.store_class,
            crate::residency::ResidencyStoreClass::T3FirehoseArchive
        );
        assert_eq!(report.region, region());
    }
}

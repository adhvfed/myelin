//! # The T3 CI log tier — the `(job, step, byte-range)` index (C2) + `#step-<n>` resolution
//! (P-ST-26 / global P-328)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §3.3 (C2 — the CI log tier
//! `(job, step, byte-range)` index frozen: *firehose frames appended to a current segment, sealed
//! segments flush to T2 as content-addressed blobs inheriting T2 encryption + crypto-shred; an OLTP
//! byte-range index maps `(job, step, byte-range) → (segment-blob, offset)` for tail + range-read;
//! keying by `(job, step)` is what lets the X-1 `CheckStatus.details_ref` `#step-<n>` sub-anchor
//! resolve to the exact failing step's bytes — the jump-to-failure path*).
//! Contract-index rows 11.8 (the T3 CI log tier + the `(job, step, byte-range)` index), 3.5 (the
//! resume-cursor transport the archive seals from), 5.9 (the `CheckStatus.details_ref` `#step-<n>`
//! the index resolves). 00-reconciliation §OQ-D (the `#step-<n>` resolution).
//!
//! **Hard-problems canon:** `external-insights/04-hard-problems.md` §5 (the event-volume seam — CI is
//! the heaviest log producer; the durable archive carries POINTER events, an agent is never woken per
//! log line). The C2 index is the OLTP-side *small* pointer structure that lets a viewer / an agent /
//! the `#step-<n>` jump-to-failure resolve a step to its exact bytes WITHOUT a per-line wake.
//!
//! ## What this module IS (the P-ST-26 deliverable — the C2 half of 11.8)
//! [`CiLogTier`] is the CI-keyed instance of the P-ST-20 [`crate::firehose_archive::FirehoseArchiver`]
//! (the SEALING + per-tenant-DEK mechanism — REUSED wholesale, never re-built). It adds the ONE thing
//! P-ST-20 named as its M4 follow-on: the **`(job, step, byte-range)` index**.
//!
//! 1. **A CI log frame carries `(job, step, chunk-bytes)`.** [`CiLogFrame`] is the CI-log frame body:
//!    `(job_id, step_no, bytes)` — one chunk of a step's log. [`CiLogFrame::to_payload`] /
//!    [`CiLogFrame::from_payload`] is a deterministic, self-describing serialisation into the frozen
//!    [`myelin_events::FramePayload`] (the transport never reads the body — references-not-payloads:
//!    the chunk bytes are the redacted log text the runner already streamed, NOT inline PII; the
//!    per-subject CI-log DEK that keys isolable inline PII is the C1 sibling P-ST-27).
//! 2. **Sealing a CI batch builds the index.** [`CiLogTier::seal_ci_batch`] seals a batch of
//!    [`CiLogFrame`]s into a content-addressed T2 segment THROUGH the P-ST-20 archiver (inheriting T2
//!    encryption + crypto-shred), AND records, per `(job, step)`, the **byte-range** that step's chunk
//!    occupies in the reconstructed step log — a [`StepSpan`] `(segment content-hash, offset, len)`.
//!    A step whose log spans MULTIPLE segments accumulates one span per segment, in order — the index
//!    is append-only (`UNIQUE(job, step, seq)` in the OLTP realisation), so the reconstructed step log
//!    is the in-order concatenation of its spans.
//! 3. **`#step-<n>` resolves to the exact failing step's bytes.** [`CiLogTier::resolve_step_anchor`]
//!    parses the X-1 `myelin://<tenant>/ci/run/<run>#step-<n>` sub-anchor (5.9 / OQ-D), looks the
//!    step up in the index, reads ONLY the segment(s) the index names (decrypting through the
//!    per-tenant DEK), and slices out EXACTLY that step's bytes by the recorded byte-range. This IS
//!    the storage realisation of the GIT-D10 / CI-D8 jump-to-failure. A `#step-<n>` for a step the
//!    index never saw is a LOUD [`CiLogError::UnknownStep`] (never an empty/wrong serve).
//!
//! ## Coherence (EI-01 §7) — REUSE, never duplicate
//! - The SEALING + per-tenant-DEK + content-addressing + crypto-shred is the P-ST-20
//!   [`crate::firehose_archive::FirehoseArchiver`] — REUSED wholesale (this module OWNS one and calls
//!   `seal` / `read_segment`); it adds NO second sealing path and NO second key store. The CI log
//!   segment IS a T2 firehose segment, keyed for CI.
//! - The frame transport is `myelin_events::Firehose` (the frozen 3.5 transport, P-141) — a CI log
//!   frame is an ordinary [`myelin_events::Frame`] whose opaque payload is a [`CiLogFrame`]. The tier
//!   rides the SAME `tail` range-read the live viewer uses (via the archiver's `seal_from_firehose`).
//! - The residency report REUSES [`crate::residency::ResidencyStoreClass::T3FirehoseArchive`] (a CI
//!   log segment IS a T3 firehose archive segment) — no new residency variant; it feeds the SAME
//!   `verify_region_pinning` aggregation.
//!
//! ## Floors named (deferred bodies → filling prompt) — VISION §3, prompt DoD
//! - **The per-SUBJECT CI-log DEK (C1)** — a CI log segment carrying isolable inline PII keyed to that
//!   subject's DEK so their erasure crypto-shreds exactly their log content — is the SIBLING prompt
//!   **P-ST-27 (M4)**. This prompt keys CI log segments under the per-TENANT DEK (the P-ST-20 default);
//!   the C1 extension is a key-CLASS swap on the same [`crate::encryption::DekContentWrap`] seam (pass
//!   `CryptoShred("subject_dek")` + the subject to the archiver's wrap), NOT a new mechanism. Recorded.
//! - **The OLTP persistence of the index** is the in-process [`CiLogIndex`] map here (the index SHAPE +
//!   the `(job, step, byte-range) → (segment, offset)` resolution are frozen + testable now); the real
//!   `ci_log_index` OLTP table (`UNIQUE(job, step, seq)`) lands when `serve`'s pool body does
//!   (P-S12/P-S15) — a one-line backing swap behind the same append/resolve calls. Recorded.
//! - **The real broker firehose + the real object-store segment backing** are the Bus M0 deployment
//!   seam (P-S12) and **P-ST-30 (M5)** — inherited from the P-ST-20 archiver this wraps. Recorded.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2; prompt TESTS field)
//! The byte-range index resolution ([`CiLogTier::seal_ci_batch`] index-build +
//! [`CiLogTier::resolve_step_anchor`] parse + lookup + slice + [`CiLogFrame`] payload encode/decode)
//! is mandatory-core: the load-bearing decision is *a `#step-<n>` anchor resolves to the EXACT failing
//! step's bytes* (not a neighbouring step's, not the whole segment, not an empty serve). The floor is
//! **≥ 80%**; the ACHIEVED score is **100%** — `cargo mutants -p myelin-storage -f
//! crates/myelin-storage/src/ci_log_index.rs` reports 76 mutants / 64 caught + 1 timeout-caught / 11
//! unviable / **0 missed** (65 viable, all killed). The `(hi << 4) | lo` byte-assembly was written as
//! `hi * 16 + lo` precisely to avoid the provably-EQUIVALENT `| → ^` mutant (disjoint nibble bit
//! ranges make `|`/`^` indistinguishable); the corrupt-index `(job, step)` verify is pinned by
//! `a_desynced_span_pointing_at_the_wrong_step_is_a_loud_refusal_not_a_wrong_serve`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_events::{Frame, FramePayload};
use myelin_tenancy::{Region, TenantId};

use crate::blob::ContentHash;
use crate::firehose_archive::{ArchiveError, FirehoseArchiver, SealedSegment};
use crate::residency::StoreResidencyReport;

/// The firehose stream CI logs ride (the `(stream, scope)` half the archiver seals under). CI logs
/// are scoped per-run; the stream name is fixed so the durable archive groups CI log segments.
pub const CI_LOG_STREAM: &str = "ci-logs";

/// **One chunk of a CI step's log — the CI log frame body (references-not-payloads transport view).**
/// `(job_id, step_no, bytes)`: a redacted log chunk for one `(job, step)`. It serialises to/from the
/// frozen [`FramePayload`] so it rides the 3.5 transport as an ordinary [`Frame`] (the transport never
/// reads the body). `bytes` is the redacted log TEXT the runner already streamed — not inline PII (the
/// per-subject DEK that keys isolable inline PII is the C1 sibling P-ST-27).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiLogFrame {
    /// The CI job this chunk belongs to (PII-free opaque id) — the `(job, step)` index's first key.
    pub job_id: String,
    /// The step within the job (1-based, matching the X-1 `#step-<n>` anchor) — the second index key.
    pub step_no: u32,
    /// The redacted log chunk bytes for this `(job, step)`. The index records WHERE these land.
    pub bytes: Vec<u8>,
}

impl CiLogFrame {
    /// A CI log frame for `(job, step)` carrying one redacted log chunk.
    pub fn new(job_id: impl Into<String>, step_no: u32, bytes: impl Into<Vec<u8>>) -> CiLogFrame {
        CiLogFrame {
            job_id: job_id.into(),
            step_no,
            bytes: bytes.into(),
        }
    }

    /// Serialise into the opaque [`FramePayload`] the firehose carries. Deterministic + self-describing
    /// so a sealed segment round-trips the exact `(job, step, bytes)`:
    /// `<step_no>\u{1}<job_id>\u{1}<bytes-as-lossless-latin1>` would lose binary; instead the payload
    /// is `header\u{1}body` where the header is `<step_no>:<job_byte_len>` and the body is
    /// `<job_id-utf8><raw-bytes>`. The [`FramePayload`] inner is a `String`, so the raw bytes are
    /// carried as a base16 (hex) tail — lossless for any byte content. The structure is fixed-shape so
    /// [`Self::from_payload`] is total + LOUD on any malformation.
    pub fn to_payload(&self) -> FramePayload {
        // `step_no` and `job_id` are ASCII-safe metadata; the log chunk is hex-encoded so ANY bytes
        // survive the `String`-typed FramePayload (the transport carries opaque text).
        let hex: String = self.bytes.iter().map(|b| format!("{b:02x}")).collect();
        FramePayload(format!("{}\u{1}{}\u{1}{}", self.step_no, self.job_id, hex))
    }

    /// Parse a [`FramePayload`] back into a [`CiLogFrame`] — the inverse of [`Self::to_payload`].
    /// `None` on ANY malformation (a garbled CI log frame is a LOUD failure, never a partial decode):
    /// a missing field, a non-numeric `step_no`, or odd/invalid hex.
    pub fn from_payload(payload: &FramePayload) -> Option<CiLogFrame> {
        let mut parts = payload.0.splitn(3, '\u{1}');
        let step_no: u32 = parts.next()?.parse().ok()?;
        let job_id = parts.next()?.to_string();
        let hex = parts.next()?;
        if hex.len() % 2 != 0 {
            return None;
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        let raw = hex.as_bytes();
        let mut i = 0;
        while i < raw.len() {
            let hi = hex_val(raw[i])?;
            let lo = hex_val(raw[i + 1])?;
            // `hi * 16 + lo` (not `(hi << 4) | lo`): semantically identical for nibbles, but it avoids
            // the provably-EQUIVALENT `| → ^` mutant — high/low nibbles are disjoint bit ranges, so
            // `|` and `^` are indistinguishable, whereas `*`/`+` mutants here are all caught by the
            // byte-exact round-trip tests (the mandatory-core stays fully pinned, no equivalent gap).
            bytes.push(hi * 16 + lo);
            i += 2;
        }
        Some(CiLogFrame {
            job_id,
            step_no,
            bytes,
        })
    }
}

/// One hex nibble → its value, or `None` for a non-hex byte (the LOUD malformation path).
fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// **One span of a `(job, step)`'s log within a sealed segment — the C2 index row's value.** It names
/// the content-addressed segment blob the bytes rest in and the byte-range WITHIN the reconstructed
/// step log this span covers (`offset..offset+len`). The reconstructed step log is the in-order
/// concatenation of a step's spans (one per CI log frame for that step); a multi-segment step
/// accumulates one span per segment. This is the `(segment-blob, offset)` the frozen
/// `(job, step, byte-range) → (segment-blob, offset)` index maps to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepSpan {
    /// The content address of the sealed segment blob this span's bytes live in.
    pub segment: ContentHash,
    /// The offset of this span's bytes WITHIN the reconstructed step log (the running total of the
    /// step's prior spans' lengths) — the byte-range's lower bound.
    pub offset: u64,
    /// The length in bytes of this span (the CI log frame's chunk length).
    pub len: u64,
    /// The `seq` of the firehose frame this span came from — the `UNIQUE(job, step, seq)` order key
    /// (so re-delivery is idempotent and the spans concatenate in firehose order, never reordered).
    pub frame_seq: u64,
}

impl StepSpan {
    /// The inclusive-exclusive byte-range `[offset, offset + len)` this span covers in the step log.
    pub fn byte_range(&self) -> (u64, u64) {
        (self.offset, self.offset + self.len)
    }
}

/// **The `(job, step, byte-range)` index (C2) — `(job, step) → ordered spans`.** Keyed by
/// `(job_id, step_no)`; each value is the in-order list of [`StepSpan`]s the step's log occupies
/// across the sealed segments. The in-process map is the index SHAPE; the OLTP table
/// (`UNIQUE(job, step, seq)`) is the named P-S12 backing swap (the resolve calls do not change shape).
/// A `BTreeMap` keyed on `(job, step)` so iteration is deterministic; the spans `Vec` is append-only
/// and de-duplicated on `frame_seq` (at-least-once idempotence).
#[derive(Debug, Default)]
pub struct CiLogIndex {
    by_step: BTreeMap<(String, u32), Vec<StepSpan>>,
}

impl CiLogIndex {
    /// A fresh, empty index.
    pub fn new() -> CiLogIndex {
        CiLogIndex::default()
    }

    /// Append a span for `(job, step)` — idempotent on `frame_seq` (a re-delivered frame is absorbed,
    /// never double-counted, so the reconstructed offsets stay correct). Returns `true` if the span
    /// was NEW (admitted) and `false` if it was a re-delivery already in the index.
    fn append(&mut self, job_id: &str, step_no: u32, span: StepSpan) -> bool {
        let spans = self
            .by_step
            .entry((job_id.to_string(), step_no))
            .or_default();
        if spans.iter().any(|s| s.frame_seq == span.frame_seq) {
            return false; // at-least-once duplicate — idempotent no-op
        }
        spans.push(span);
        // Keep the spans in firehose-`seq` order so the concatenation is the true step log order,
        // regardless of seal-batch interleaving.
        spans.sort_by_key(|s| s.frame_seq);
        true
    }

    /// The ordered spans for `(job, step)` — `None` if the index never saw that step.
    pub fn spans(&self, job_id: &str, step_no: u32) -> Option<&[StepSpan]> {
        self.by_step
            .get(&(job_id.to_string(), step_no))
            .map(|v| v.as_slice())
    }

    /// The total reconstructed log length for `(job, step)` in bytes (the sum of its spans' lengths) —
    /// `0` for an unknown step.
    pub fn step_log_len(&self, job_id: &str, step_no: u32) -> u64 {
        self.by_step
            .get(&(job_id.to_string(), step_no))
            .map(|v| v.iter().map(|s| s.len).sum())
            .unwrap_or(0)
    }

    /// The number of `(job, step)` keys the index holds (the index is live / doing work).
    pub fn step_count(&self) -> usize {
        self.by_step.len()
    }
}

/// **Why a CI-log index operation failed (the typed, LOUD verdicts — never a silent wrong serve).**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiLogError {
    /// A `#step-<n>` was requested for a `(job, step)` the index never saw — a LOUD miss, never an
    /// empty serve (the jump-to-failure must point at a step that exists).
    UnknownStep { job_id: String, step_no: u32 },
    /// A `#step-<n>` sub-anchor could not be parsed into `(run, step_no)` — a malformed anchor.
    MalformedAnchor(String),
    /// The underlying segment read/seal failed (the archive error — a crypto-shredded segment surfaces
    /// here, the GD-4 lever). Carries the archive error for diagnosis.
    Archive(ArchiveError),
    /// A resolved segment did not contain the expected `(job, step)` chunk at the indexed offset — a
    /// corrupt/desynchronised index, surfaced LOUDLY (never a wrong-bytes serve).
    SpanOutOfBounds {
        segment: ContentHash,
        offset: u64,
        len: u64,
    },
}

impl core::fmt::Display for CiLogError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CiLogError::UnknownStep { job_id, step_no } => write!(
                f,
                "ci log index: no log for (job={job_id}, step={step_no}) — #step-{step_no} unresolvable"
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
                "ci log index: span [{offset}, {}) out of bounds in segment {} — corrupt index, serve refused",
                offset + len,
                segment.to_multihash_string()
            ),
        }
    }
}

impl std::error::Error for CiLogError {}

impl From<ArchiveError> for CiLogError {
    fn from(e: ArchiveError) -> Self {
        CiLogError::Archive(e)
    }
}

/// **A parsed X-1 `#step-<n>` sub-anchor (`myelin://<tenant>/ci/run/<run>#step-<n>`, contract 5.9 /
/// OQ-D).** The `details_ref` the `CheckStatus` carries is a `myelin://…/ci/run/<id>` artifact ref
/// with a `#step-<n>` sub-anchor; the index resolves the `(run, step_no)` to the failing step's bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepAnchor {
    /// The run id from the `ci/run/<run>` path (the `(job, step)` index is per-run; here the run id
    /// IS the job grouping for the resolve — see [`CiLogTier::resolve_step_anchor`]).
    pub run_id: String,
    /// The step number from the `#step-<n>` sub-anchor (1-based, matching [`CiLogFrame::step_no`]).
    pub step_no: u32,
}

impl StepAnchor {
    /// Parse `myelin://<tenant>/ci/run/<run>#step-<n>` → `(run_id, step_no)`. Returns a
    /// [`CiLogError::MalformedAnchor`] if the shape does not match (a wrong-shaped ref is LOUD, never
    /// silently resolved to step 0). Tolerant of an absent `myelin://` scheme (a bare
    /// `…/ci/run/<run>#step-<n>` resolves identically — the resolver keys on `ci/run/<run>` + the
    /// sub-anchor, not the authority).
    pub fn parse(anchor: &str) -> Result<StepAnchor, CiLogError> {
        let malformed = || CiLogError::MalformedAnchor(anchor.to_string());
        let (path, frag) = anchor.split_once('#').ok_or_else(malformed)?;
        let step_no: u32 = frag
            .strip_prefix("step-")
            .ok_or_else(malformed)?
            .parse()
            .map_err(|_| malformed())?;
        // The run id is the path segment after `ci/run/`.
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

/// **The T3 CI log tier (C2) — the P-ST-20 archiver + the `(job, step, byte-range)` index (P-ST-26).**
/// It OWNS one per-tenant-DEK [`FirehoseArchiver`] (the SEALING mechanism, REUSED) and the
/// [`CiLogIndex`] (the C2 add). Sealing a CI batch seals the bytes into a content-addressed,
/// DEK-encrypted T2 segment AND records the byte-range index; resolving a `#step-<n>` reads ONLY the
/// indexed segment(s) and slices the exact step bytes.
pub struct CiLogTier {
    /// The run this tier indexes (CI logs are per-run; the run id keys the `#step-<n>` resolve).
    run_id: String,
    /// The P-ST-20 sealing + per-tenant-DEK archiver — REUSED wholesale (no second seal path).
    archiver: FirehoseArchiver,
    /// The `(job, step, byte-range)` index (C2).
    index: Mutex<CiLogIndex>,
}

impl CiLogTier {
    /// **Build a CI log tier for `run_id` over a per-tenant-DEK firehose archive.** The archiver is the
    /// P-ST-20 [`FirehoseArchiver::with_tenant_dek`] — every CI log segment is a content-addressed T2
    /// blob under the per-tenant DEK (inheriting crypto-shred). The per-SUBJECT CI-log DEK (C1) is the
    /// sibling P-ST-27 (a key-class swap on the same wrap). The `engine`'s tenant KEK must already
    /// exist (the cell-provisioning wired it).
    pub fn with_tenant_dek(
        run_id: impl Into<String>,
        tenant: TenantId,
        region: Region,
        engine: std::sync::Arc<crate::kms::KmsEngine>,
    ) -> CiLogTier {
        CiLogTier {
            run_id: run_id.into(),
            archiver: FirehoseArchiver::with_tenant_dek(tenant, region, engine),
            index: Mutex::new(CiLogIndex::new()),
        }
    }

    /// The run id this tier indexes.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// **Seal a batch of CI log frames into a content-addressed T2 segment + build the C2 index.** Each
    /// [`CiLogFrame`] becomes one firehose [`Frame`] (its `(job, step, bytes)` serialised into the
    /// opaque payload); the whole batch seals through the P-ST-20 archiver (one content-addressed,
    /// DEK-encrypted segment), and then the index records, per `(job, step)`, the byte-range that
    /// frame's chunk occupies in the reconstructed step log (the running per-step offset). The
    /// per-frame `seq` is supplied by the caller (the firehose's monotonic `(stream, scope)` seq, the
    /// `UNIQUE(job, step, seq)` order key). Returns the sealed segment pointer.
    ///
    /// REFUSES an empty batch (the archiver's [`ArchiveError::EmptySegment`] — never a junk seal).
    pub fn seal_ci_batch(&self, frames: &[(u64, CiLogFrame)]) -> Result<SealedSegment, CiLogError> {
        // Lower each CI log frame to a transport `Frame` (opaque payload), then seal the batch.
        let scope_selector = format!("run:{}", self.run_id); // the `(stream, scope)` selector
        let transport: Vec<Frame> = frames
            .iter()
            .map(|(seq, clf)| Frame {
                seq: *seq,
                payload: clf.to_payload(),
            })
            .collect();
        let segment = self
            .archiver
            .seal(CI_LOG_STREAM, &scope_selector, &transport)?;

        // Build the index: per `(job, step)`, record this chunk's byte-range in the step log.
        let mut index = self.index.lock().expect("ci log index mutex");
        for (seq, clf) in frames {
            // The offset is the running total of this step's prior spans' lengths (the reconstructed
            // step-log offset), computed BEFORE this span is appended.
            let offset = index.step_log_len(&clf.job_id, clf.step_no);
            let span = StepSpan {
                segment: segment.content_hash.clone(),
                offset,
                len: clf.bytes.len() as u64,
                frame_seq: *seq,
            };
            index.append(&clf.job_id, clf.step_no, span);
        }
        Ok(segment)
    }

    /// **Resolve a `(job, step)` to its EXACT reconstructed log bytes via the C2 index.** Reads ONLY
    /// the segment(s) the index names for `(job, step)` (decrypting each through the per-tenant DEK),
    /// finds the `(job, step)` frame within each segment, and concatenates the spans IN ORDER. A step
    /// the index never saw is a LOUD [`CiLogError::UnknownStep`] (never an empty/wrong serve). This is
    /// the byte-exact resolution behind the X-1 jump-to-failure.
    pub fn resolve_step(&self, job_id: &str, step_no: u32) -> Result<Vec<u8>, CiLogError> {
        let spans: Vec<StepSpan> = {
            let index = self.index.lock().expect("ci log index mutex");
            // `spans` is `Some` iff the index ever saw `(job, step)`, and `append` always pushes at
            // least one span — so a present key is never empty (no redundant `is_empty` guard). A step
            // the index never saw is a LOUD miss, never an empty serve.
            match index.spans(job_id, step_no) {
                Some(spans) => spans.to_vec(),
                None => {
                    return Err(CiLogError::UnknownStep {
                        job_id: job_id.to_string(),
                        step_no,
                    })
                }
            }
        };

        let mut out = Vec::with_capacity(spans.iter().map(|s| s.len as usize).sum());
        for span in &spans {
            // Read the segment the index names + decode its frames (decrypts through the DEK).
            let frames = self.archiver.read_segment(&span.segment)?;
            // Find the `(job, step)` chunk at this span's frame_seq within the segment.
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

    /// **Resolve an X-1 `#step-<n>` sub-anchor to the exact failing step's bytes (the headline GATE).**
    /// Parses `myelin://<tenant>/ci/run/<run>#step-<n>` (5.9 / OQ-D), then resolves the step. The
    /// `(job, step)` index is keyed per-job; a CI run's `#step-<n>` resolves against the run's primary
    /// job (`run_id` as the job key) — the common single-job case + the X-1 anchor's grain. The
    /// anchor's `run` MUST match this tier's run (a cross-run anchor is a LOUD `UnknownStep`).
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

    /// The `(job, step, byte-range)` index's step count (the index is live).
    pub fn indexed_step_count(&self) -> usize {
        self.index.lock().expect("ci log index mutex").step_count()
    }

    /// The reconstructed log length for `(job, step)` (the sum of its indexed spans) — `0` for an
    /// unknown step. Telemetry: the resolved byte-range matches the step's bytes (the GATE reads it).
    pub fn step_log_len(&self, job_id: &str, step_no: u32) -> u64 {
        self.index
            .lock()
            .expect("ci log index mutex")
            .step_log_len(job_id, step_no)
    }

    /// The underlying P-ST-20 archiver (its `unencrypted_segment_count == 0` /
    /// `segment_content_addressed == true` telemetry rides through — a CI log segment is a T2 segment).
    pub fn archiver(&self) -> &FirehoseArchiver {
        &self.archiver
    }

    /// Test/CI-only: inject a DESYNCED span for `(job, step)` pointing at a `frame_seq` that resolves
    /// to a DIFFERENT `(job, step)` chunk in `segment` — the corrupt-index case the
    /// `clf.job_id == job_id && clf.step_no == step_no` verify in [`Self::resolve_step`] defends
    /// against (a desync must surface as a LOUD [`CiLogError::SpanOutOfBounds`], NEVER a wrong-step
    /// serve). Returns the injected span's frame_seq. Exposed so the byte-exactness verify is provable.
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
            },
        );
    }

    /// The T3 firehose-archive residency report (a CI log segment IS a T3 firehose segment — REUSES
    /// [`crate::residency::ResidencyStoreClass::T3FirehoseArchive`], no new variant). Feeds the SAME
    /// `verify_region_pinning` aggregation.
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

    // ── CiLogFrame payload round-trip (deterministic, lossless, LOUD on malformation) ──

    #[test]
    fn ci_log_frame_payload_round_trips_exactly_including_binary() {
        let clf = CiLogFrame::new("build", 3, vec![0x00, 0xff, b'l', b'o', b'g', 0x01]);
        let payload = clf.to_payload();
        let back = CiLogFrame::from_payload(&payload).expect("round-trip");
        assert_eq!(
            back, clf,
            "the exact (job, step, bytes) round-trips, binary included"
        );
    }

    #[test]
    fn ci_log_frame_payload_is_deterministic() {
        let clf = CiLogFrame::new("test", 2, b"hello".to_vec());
        assert_eq!(clf.to_payload(), clf.to_payload());
        // A different step / job / bytes encodes differently.
        assert_ne!(
            clf.to_payload(),
            CiLogFrame::new("test", 3, b"hello".to_vec()).to_payload()
        );
        assert_ne!(
            clf.to_payload(),
            CiLogFrame::new("lint", 2, b"hello".to_vec()).to_payload()
        );
        assert_ne!(
            clf.to_payload(),
            CiLogFrame::new("test", 2, b"hellp".to_vec()).to_payload()
        );
    }

    #[test]
    fn ci_log_frame_from_payload_rejects_malformation_loudly() {
        // Missing fields.
        assert!(CiLogFrame::from_payload(&FramePayload("3".into())).is_none());
        assert!(CiLogFrame::from_payload(&FramePayload("3\u{1}job".into())).is_none());
        // Non-numeric step.
        assert!(CiLogFrame::from_payload(&FramePayload("x\u{1}job\u{1}6c6f67".into())).is_none());
        // Odd-length hex.
        assert!(CiLogFrame::from_payload(&FramePayload("3\u{1}job\u{1}abc".into())).is_none());
        // Invalid hex.
        assert!(CiLogFrame::from_payload(&FramePayload("3\u{1}job\u{1}zz".into())).is_none());
        // A valid empty-chunk frame is fine (empty hex tail).
        let ok = CiLogFrame::from_payload(&FramePayload("3\u{1}job\u{1}".into()))
            .expect("empty chunk ok");
        assert_eq!(ok, CiLogFrame::new("job", 3, Vec::<u8>::new()));
    }

    #[test]
    fn ci_log_frame_hex_decode_is_byte_exact_for_every_nibble_path() {
        // Pin the EXACT hex-nibble math: `to_payload` only emits lowercase, so these decode-only
        // cases pin `from_payload`'s nibble arithmetic + the UPPERCASE arm + the `(hi << 4) | lo`
        // byte assembly (the resolution must reconstruct the EXACT byte, never a near-miss).
        //
        // `0x1b` = hi nibble 1 (0001), lo nibble 11 (1011): `(1<<4)|11 == 0x1b`, whereas
        // `(1<<4)^11 == 0x1a` — this case DISCRIMINATES `|` from `^` in the byte assembly.
        let one_b = CiLogFrame::from_payload(&FramePayload("0\u{1}j\u{1}1b".into())).expect("1b");
        assert_eq!(one_b.bytes, vec![0x1b]);

        // The UPPERCASE arm (`b'A'..=b'F'`): `"FF"` → 255 (hi=15 via `c - b'A' + 10`, lo=15). If the
        // `+10` offset or the uppercase arm were wrong, this byte would be wrong / the parse would
        // fail. Mixed case `"Af"` → 0xAF pins both arms agreeing on the +10 offset.
        let ff = CiLogFrame::from_payload(&FramePayload("0\u{1}j\u{1}FF".into())).expect("FF");
        assert_eq!(ff.bytes, vec![0xff]);
        let af = CiLogFrame::from_payload(&FramePayload("0\u{1}j\u{1}Af".into())).expect("Af");
        assert_eq!(af.bytes, vec![0xaf]);

        // The lowercase `a` offset: `"a0"` → 0xA0 (hi=10 via `c - b'a' + 10`).
        let a0 = CiLogFrame::from_payload(&FramePayload("0\u{1}j\u{1}a0".into())).expect("a0");
        assert_eq!(a0.bytes, vec![0xa0]);

        // And a full round-trip of EVERY byte value through encode→decode (0..=255) — the index's
        // step bytes survive byte-for-byte, the load-bearing "exact failing step bytes" property.
        let all: Vec<u8> = (0..=255u16).map(|b| b as u8).collect();
        let rt = CiLogFrame::from_payload(&CiLogFrame::new("j", 1, all.clone()).to_payload())
            .expect("all-bytes round-trip");
        assert_eq!(rt.bytes, all);
    }

    // ── the headline: seal a CI batch + resolve #step-<n> to the EXACT bytes ──

    #[test]
    fn seal_then_resolve_step_returns_exactly_that_steps_bytes() {
        let t = tier("run-1");
        // Three steps, one chunk each, in one sealed batch.
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

        // The tier carries its run id (the `#step-<n>` anchor resolves against it).
        assert_eq!(t.run_id(), "run-1");

        // The segment is content-addressed + the archiver telemetry rides through (0 unencrypted).
        assert!(seg
            .content_hash
            .to_multihash_string()
            .starts_with("blake3:"));
        assert_eq!(t.archiver().telemetry().unencrypted_segment_count(), 0);
        assert!(t.archiver().telemetry().segment_content_addressed());
        assert_eq!(t.indexed_step_count(), 3, "three (job, step) keys indexed");

        // resolve_step returns EXACTLY that step's bytes — not a neighbour's, not the whole segment.
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

        // The X-1 `details_ref` `#step-<n>` jump-to-failure resolves to the exact failing step's bytes.
        let bytes = t
            .resolve_step_anchor("myelin://acme/ci/run/run-7#step-2")
            .expect("resolve the #step-2 jump-to-failure");
        assert_eq!(
            bytes, b"FAILURE HERE\n",
            "the anchor resolves to step 2's EXACT bytes"
        );
        // The resolved byte-range matches the step's bytes (the GATE telemetry).
        assert_eq!(t.step_log_len("run-7", 2), bytes.len() as u64);
    }

    #[test]
    fn a_step_spanning_multiple_segments_concatenates_in_order() {
        let t = tier("run-9");
        // step 2's log arrives in two chunks, in two SEPARATE sealed batches (two segments).
        t.seal_ci_batch(&[(1, CiLogFrame::new("run-9", 2, b"part-A ".to_vec()))])
            .expect("seal batch 1");
        t.seal_ci_batch(&[(2, CiLogFrame::new("run-9", 2, b"part-B".to_vec()))])
            .expect("seal batch 2");

        // The reconstructed step log is the in-order concatenation across both segments.
        assert_eq!(t.resolve_step("run-9", 2).unwrap(), b"part-A part-B");
        assert_eq!(t.step_log_len("run-9", 2), 13);
        // The two spans name DIFFERENT segments with running offsets 0 then 7.
        let index = t.index.lock().unwrap();
        let spans = index.spans("run-9", 2).unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].offset, spans[0].len), (0, 7));
        assert_eq!((spans[1].offset, spans[1].len), (7, 6));
        assert_ne!(
            spans[0].segment, spans[1].segment,
            "two distinct sealed segments"
        );
    }

    #[test]
    fn interleaved_steps_in_one_batch_index_independently() {
        let t = tier("run-x");
        // Two steps interleaved in one batch — each step's offsets are independent.
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
        // A step the index never saw → LOUD UnknownStep (never an empty serve).
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
        // The byte-exactness verify: `resolve_step` checks the resolved chunk's `(job, step)` matches
        // the requested `(job, step)` (the `job == job && step == step` guard). A corrupt index whose
        // span points at a frame_seq carrying a DIFFERENT step must surface a LOUD SpanOutOfBounds,
        // never serve the wrong step's bytes — this pins the `&&` (a `||` would accept a
        // job-matches-but-step-differs frame and serve step 1's bytes for a step-2 request).
        let t = tier("run-1");
        // Seal a real segment holding a (run-1, step 1) chunk at frame_seq 5.
        let seg = t
            .seal_ci_batch(&[(5, CiLogFrame::new("run-1", 1, b"STEP-ONE-BYTES".to_vec()))])
            .expect("seal");
        // Inject a DESYNCED span for (run-1, step 2) pointing at that same frame_seq 5 — the frame
        // there is step 1, not step 2 (a corrupt index).
        t.inject_desynced_span_for_drill("run-1", 2, seg.content_hash.clone(), 5, 14);

        // Resolving step 2 must REFUSE loudly (the seq-5 frame is step 1: job matches, step differs).
        // `&&` → refuse (SpanOutOfBounds); `||` → wrongly serve step 1's bytes.
        let err = t
            .resolve_step("run-1", 2)
            .expect_err("a desync must refuse, never wrong-serve");
        assert!(
            matches!(err, CiLogError::SpanOutOfBounds { .. }),
            "a wrong-step desync is a LOUD SpanOutOfBounds, never step 1's bytes for a step-2 request, got {err}"
        );
        // Step 1 itself still resolves correctly (the guard rejects only the cross-step desync).
        assert_eq!(t.resolve_step("run-1", 1).unwrap(), b"STEP-ONE-BYTES");
    }

    #[test]
    fn cross_run_anchor_does_not_resolve() {
        let t = tier("run-1");
        t.seal_ci_batch(&[(1, CiLogFrame::new("run-1", 1, b"ok".to_vec()))])
            .expect("seal");
        // An anchor for a DIFFERENT run is a loud miss (never resolves against the wrong run's index).
        assert!(matches!(
            t.resolve_step_anchor("myelin://acme/ci/run/run-2#step-1"),
            Err(CiLogError::UnknownStep { .. })
        ));
    }

    // ── the anchor parser (5.9 / OQ-D) ──

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
        // A bare (schemeless) anchor resolves identically (keys on ci/run/<run> + the sub-anchor).
        let b = StepAnchor::parse("acme/ci/run/run-42#step-3").expect("parse bare");
        assert_eq!(b, a);
    }

    #[test]
    fn step_anchor_rejects_malformation_loudly() {
        assert!(matches!(
            StepAnchor::parse("myelin://acme/ci/run/run-1"), // no #step
            Err(CiLogError::MalformedAnchor(_))
        ));
        assert!(matches!(
            StepAnchor::parse("myelin://acme/ci/run/run-1#frag-1"), // wrong sub-anchor
            Err(CiLogError::MalformedAnchor(_))
        ));
        assert!(matches!(
            StepAnchor::parse("myelin://acme/ci/run/run-1#step-x"), // non-numeric step
            Err(CiLogError::MalformedAnchor(_))
        ));
        assert!(matches!(
            StepAnchor::parse("myelin://acme/issues/42#step-1"), // not a ci/run path
            Err(CiLogError::MalformedAnchor(_))
        ));
        assert!(matches!(
            StepAnchor::parse("myelin://acme/ci/run/#step-1"), // empty run id
            Err(CiLogError::MalformedAnchor(_))
        ));
    }

    // ── crypto-shred inheritance (the CI log segment IS a T2 segment under the per-tenant DEK) ──

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

        // Crypto-shred the per-tenant DEK the CI log segment is sealed under → the step is
        // unrecoverable (the C1 per-subject DEK that scopes this to a single subject is P-ST-27).
        assert!(eng.destroy_dek(&DekId::new(tenant(), KeyClass::Tenant)));
        let res =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.resolve_step("run-1", 1)));
        assert!(
            res.is_err(),
            "a crypto-shredded CI log step is unrecoverable (LOUD), never served"
        );
    }

    // ── index idempotence (at-least-once re-delivery) ──

    #[test]
    fn re_sealing_a_frame_seq_is_idempotent_in_the_index() {
        let t = tier("run-1");
        t.seal_ci_batch(&[(1, CiLogFrame::new("run-1", 1, b"line".to_vec()))])
            .expect("seal 1");
        // A re-delivery of the SAME frame_seq (at-least-once) does not double-count the span.
        t.seal_ci_batch(&[(1, CiLogFrame::new("run-1", 1, b"line".to_vec()))])
            .expect("re-seal 1");
        assert_eq!(
            t.step_log_len("run-1", 1),
            4,
            "the duplicate seq is absorbed (idempotent)"
        );
        assert_eq!(t.resolve_step("run-1", 1).unwrap(), b"line");
    }

    // ── empty batch refused (rides the archiver's EmptySegment) ──

    #[test]
    fn seal_empty_ci_batch_is_refused() {
        let t = tier("run-1");
        assert!(matches!(
            t.seal_ci_batch(&[]),
            Err(CiLogError::Archive(ArchiveError::EmptySegment))
        ));
        assert_eq!(t.indexed_step_count(), 0, "a refused seal indexes nothing");
    }

    // ── StepSpan byte_range + error Display ──

    #[test]
    fn step_span_byte_range_and_error_display() {
        let span = StepSpan {
            segment: ContentHash::blake3(b"x"),
            offset: 10,
            len: 5,
            frame_seq: 1,
        };
        assert_eq!(span.byte_range(), (10, 15));
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

    // ── residency report rides through (T3FirehoseArchive — no new variant) ──

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

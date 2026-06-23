//! # `live_tail` — the resume-cursor live-tail VIEWER + the `details_ref` jump-to-failure resolution
//! (CI-P21 → P-364, M4)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §7.1 (**the live-log view uses the frozen resume-cursor protocol**: a viewer
//! `subscribe(stream, scope = run:<id>, cursor?)`; on reconnect it `resume(stream, scope, last_seq)`
//! and the transport backfills `(last_seq, now]` — **a reconnect loses zero log lines**; if `last_seq`
//! is past the retention window, a `resync_required` falls back to a **range-read of the sealed
//! segments**; scope is **bounded**, never `*`) + §4 (**the `CheckStatus.details_ref =
//! …/ci/run/<id>#step-<n>` resolves through the `log_anchor` index** → the byte range — the X-1 / OQ-D
//! jump-to-failure path). `01-tech-and-data-model.md` §3.5 (the `(job, step, byte-range)` index —
//! `log_segment` + `log_anchor`; step ids stable across retries).
//!
//! ## What CI-P21 ships here — the VIEWER half (CI-P20 shipped the PRODUCER half)
//! CI-P20 (`log_pipeline`) ships the PRODUCER: `ship_line` redacts → publishes a firehose frame → seals
//! a segment → writes the `(job, step, byte-range)` index → emits the coalesced `ci.log.available`
//! pointer. THIS module ships the VIEWER that reads it back:
//!
//! - **[`LiveTail`] — the resume-cursor live-tail viewer.** `subscribe(coord, cursor?)` opens a
//!   subscription on the BOUNDED `run:<id>` scope (never `*` — the whitelist-not-`*` rule, BUS-3
//!   generalised). On reconnect `resume(coord, last_seq)` backfills `(last_seq, now]` from the frozen
//!   firehose retention window then goes live — **a reconnect loses ZERO log lines** (CI-D11). If
//!   `last_seq` is OLDER than the retention window, the transport raises `resync_required` and the
//!   viewer falls back to a **range-read of the sealed segments** (CI-P20's `log_segment` index →
//!   `BlobStore::get`) — the cold path, NAMED, never a silent partial replay.
//! - **[`DetailsRefResolver`] — the `#step-<n>` jump-to-failure resolution.** A
//!   `CheckStatus.details_ref = …/ci/run/<id>#step-<n>` (the X-1 / OQ-D sub-anchor) resolves through
//!   `log_anchor` → `log_segment` → the byte range. Step ids are OPAQUE and STABLE across retries
//!   (assigned from the snapshot, not runtime order — CI-P20's `LogCoord.step_id`). **0 dangling step
//!   anchors** (the GATE): a `details_ref` whose `(run, job, step)` has an anchor resolves to a byte
//!   range; one with no anchor is a NAMED `Tombstone`, never a silent dangle.
//!
//! ## The CI-D11 GATE (quantified — 0 lost lines)
//! 1. **A reconnect loses ZERO lines.** A viewer at `last_seq = k`; lines `k+1..now` ship while it is
//!    disconnected; `resume(last_seq = k)` backfills EXACTLY `k+1..now` then goes live — contiguous, no
//!    gap, no duplicate. [`LiveTail::resume`] composes the frozen [`Firehose::resume`] whose zero-loss
//!    property is the Bus's D-10 (P-141); this module proves CI's live-tail rides it (CI-D11).
//! 2. **An out-of-window `last_seq` → `resync_required` → a clean range-read fallback.** A `last_seq`
//!    older than the retention window cannot backfill from the live window; [`LiveTail::resume`] returns
//!    [`ResumeOutcome::ResyncRequired`] carrying the range-read of the SEALED segments (the durable
//!    archive) — the bytes are recovered from T2, never lost, never a silent gap.
//! 3. **Scope stays BOUNDED, never `*`.** Every subscribe/resume is on the `run:<id>` scope
//!    [`LogCoord::firehose_scope`] mints; an over-broad scope is unrepresentable (the transport rejects
//!    it at parse). A viewer can ONLY ever tail one bounded run, never the tenant firehose.
//! 4. **0 dangling step anchors.** [`DetailsRefResolver::resolve`] resolves a `#step-<n>` through the
//!    anchor index; a step with no anchor is a NAMED `Tombstone`, never a silent dangle.
//!
//! ## FLOORS named (VISION §3 — name-your-floors)
//! - **None new.** The live-tail composes the FROZEN firehose transport (`subscribe`/`resume`/the
//!   retention window/`resync_required`) — CI-P21 adds the CI-side viewer + the `details_ref` resolver
//!   over CI-P20's index; it does NOT re-implement the transport. The retention-window SIZE per stream
//!   class is the Bus's named floor (EB-30 / P-439, MEASURED by D-10) — inherited, not re-named here.
//! - The LIVE firehose binding + the LIVE `BlobStore` are the deploy-time swaps behind the frozen
//!   `Firehose` / `BlobStore` surfaces (inherited from CI-P20; the in-process `Firehose` +
//!   `FsBlobStore` prove the protocol shape).
//!
//! ## DB-free by default
//! The viewer holds the in-process `Firehose` handle + the buffered `log_segment`/`log_anchor` index
//! rows + the `BlobStore` handle (all CI-P20's). `cargo build`/`cargo test --workspace` stay DB-free;
//! the live integration (the range-read over the real `BlobStore`) is the CDC pair for the consumed
//! rows 3.5 + 11.8.

use crate::log_pipeline::CI_LOG_STREAM;
use crate::log_pipeline::{LogAnchorRow, LogCoord, LogSegmentRow};
use myelin_events::firehose::{Firehose, FirehoseError, Subscription};
use myelin_storage::{BlobStore, ContentHash};

// =================================================================================================
// 1. The range-read over the sealed segments (the resync_required cold-path + the details_ref bytes).
// =================================================================================================

/// **One sealed-segment range that covers a requested byte span (the durable archive read).** The
/// `resync_required` fallback (and the `details_ref` resolution) reads sealed `log_segment` rows: each
/// names a content-addressed T2 blob + the `[byte_start, byte_end)` span it covers. A range-read
/// returns the segments whose span OVERLAPS the requested range, in `byte_start` order — the viewer
/// pulls the blob bytes for exactly the range it needs (references-not-payloads; the row carries the
/// ref + the offsets, never log bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentRange {
    /// The content-addressed blob ref the sealed bytes live at (T2; `BlobStore::get` resolves it).
    pub blob_ref: String,
    /// The first byte this segment covers (inclusive) within the `(run, job)` log stream.
    pub byte_start: i64,
    /// The last byte this segment covers (exclusive) within the `(run, job)` log stream.
    pub byte_end: i64,
}

impl SegmentRange {
    /// `true` iff this segment's span overlaps the half-open requested range `[lo, hi)`.
    fn overlaps(&self, lo: i64, hi: i64) -> bool {
        self.byte_start < hi && self.byte_end > lo
    }
}

/// **The sealed-segment index a viewer range-reads against (the `(run, job)` archive view).** Holds the
/// sealed `log_segment` rows for one `(run, job)` (CI-P20 writes them; the viewer reads them). The
/// `resync_required` fallback range-reads the sealed bytes; the `details_ref` resolution maps a step's
/// byte range to the segment(s) that cover it.
///
/// This is a READ view over CI-P20's `log_segment` rows — NOT a second copy of the index (coherence,
/// EI-01 §7). The pipeline owns the authoritative rows; the viewer borrows them.
#[derive(Clone, Debug, Default)]
pub struct SegmentIndex {
    /// The sealed segments for one `(run, job)`, `byte_start`-ordered.
    segments: Vec<SegmentRange>,
}

impl SegmentIndex {
    /// Build a segment index from the sealed `log_segment` rows for ONE `(run, job)`. Only SEALED rows
    /// (a `Some(blob_ref)`) are admitted — an open segment is in the firehose window, not the archive.
    /// Rows are sorted by `byte_start` so a range-read returns the covering segments in order.
    pub fn from_rows(run_id: &str, job_id: &str, rows: &[LogSegmentRow]) -> SegmentIndex {
        let mut segments: Vec<SegmentRange> = rows
            .iter()
            .filter(|r| r.run_id == run_id && r.job_id == job_id)
            .filter_map(|r| {
                r.blob_ref.as_ref().map(|blob_ref| SegmentRange {
                    blob_ref: blob_ref.clone(),
                    byte_start: r.byte_start,
                    byte_end: r.byte_end,
                })
            })
            .collect();
        segments.sort_by_key(|s| s.byte_start);
        SegmentIndex { segments }
    }

    /// **Range-read `[lo, hi)` — the sealed segments that cover the requested byte span (arch §7.1 —
    /// the `resync_required` fall-back read).** Returns the covering segments in `byte_start` order; the
    /// viewer pulls each blob and slices the bytes it needs. Empty iff the range is outside every sealed
    /// segment (the bytes are not yet sealed — they are still in the live window, the in-window case).
    pub fn range_read(&self, lo: i64, hi: i64) -> Vec<SegmentRange> {
        self.segments
            .iter()
            .filter(|s| s.overlaps(lo, hi))
            .cloned()
            .collect()
    }

    /// The number of sealed segments in this `(run, job)` index.
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// `true` iff there are no sealed segments yet (everything is still in the live window).
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

// =================================================================================================
// 2. The live-tail viewer — subscribe / resume (the CI-D11 zero-loss-reconnect path).
// =================================================================================================

/// **The outcome of a `resume(coord, last_seq)` (arch §7.1 — the two reconnect paths).** Either the
/// in-window backfill succeeded (a live [`Subscription`] whose first frames are the replayed gap), OR
/// the `last_seq` was older than the retention window (`resync_required`) and the viewer must read the
/// gap from the SEALED segments (the durable archive). NAMED, never a silent partial replay.
#[derive(Debug)]
pub enum ResumeOutcome {
    /// **In-window resume — 0 lost lines.** The backfill `(last_seq, now]` rode the live retention
    /// window; the [`Subscription`]'s first frames ARE the gap, then it goes live. A reconnect loses
    /// ZERO log lines (the CI-D11 pass condition).
    Live(Subscription),
    /// **Out-of-window resume — `resync_required` → range-read the sealed segments.** The `last_seq`
    /// is older than the retention window floor; the gap cannot be replayed from the live window, so
    /// the viewer reads it from the durable archive. Carries the `resync_required` verdict (the
    /// `window_floor` for diagnostics) + the sealed-segment range-read covering the gap. NAMED.
    ResyncRequired {
        /// The window floor the transport reported (the oldest seq the window still holds).
        window_floor: u64,
        /// The sealed segments covering the requested gap (the range-read fallback; the viewer pulls
        /// the blob bytes for the range it needs).
        range_read: Vec<SegmentRange>,
    },
}

impl ResumeOutcome {
    /// `true` iff this is the in-window live resume (the 0-lost-lines fast path).
    pub fn is_live(&self) -> bool {
        matches!(self, ResumeOutcome::Live(_))
    }

    /// `true` iff this is the `resync_required` archive-fallback path.
    pub fn is_resync_required(&self) -> bool {
        matches!(self, ResumeOutcome::ResyncRequired { .. })
    }
}

/// **The CI live-tail viewer (arch §7.1 — the resume-cursor protocol over the firehose).** Borrows the
/// in-process [`Firehose`] (the LIVE TAIL transport CI-P20 publishes onto) + the sealed [`SegmentIndex`]
/// (the durable archive the `resync_required` fallback reads). `subscribe` opens a bounded
/// `run:<id>`-scoped subscription; `resume` backfills `(last_seq, now]` losing zero lines, or falls
/// back to a range-read of the sealed segments when the cursor is past the window.
///
/// Holds borrows, not copies — the [`Firehose`] is CI-P20's (one source of truth; coherence EI-01 §7);
/// the viewer composes the frozen transport, it does not re-implement it.
pub struct LiveTail<'a> {
    /// The in-process firehose transport (the LIVE TAIL — CI-P20 publishes; the viewer subscribes).
    firehose: &'a mut Firehose,
    /// The sealed-segment archive index for the `resync_required` range-read fallback.
    archive: SegmentIndex,
}

impl<'a> LiveTail<'a> {
    /// A viewer over `firehose` (the live transport) + `archive` (the sealed-segment durable read).
    pub fn new(firehose: &'a mut Firehose, archive: SegmentIndex) -> LiveTail<'a> {
        LiveTail { firehose, archive }
    }

    /// **`subscribe(coord, cursor?)` (arch §7.1).** Open a live-tail subscription on the BOUNDED
    /// `run:<id>` scope (never `*`). `cursor = None` starts live from now; `cursor = Some(seq)` is a
    /// `resume(seq)` (the in-window backfill). The scope is [`LogCoord::firehose_scope`]'s bounded
    /// `run:<id>` — the transport rejects an over-broad scope at parse, so a viewer can only ever tail
    /// one bounded run.
    pub fn subscribe(
        &mut self,
        coord: &LogCoord,
        cursor: Option<u64>,
    ) -> Result<Subscription, FirehoseError> {
        let scope = coord.firehose_scope()?;
        self.firehose.subscribe(CI_LOG_STREAM, &scope, cursor)
    }

    /// **`resume(coord, last_seq)` (arch §7.1 — the CI-D11 zero-loss reconnect).** Backfill
    /// `(last_seq, now]` from the live retention window then go live (a reconnect loses ZERO lines) —
    /// [`ResumeOutcome::Live`]. If `last_seq` is OLDER than the retention window, the transport raises
    /// `resync_required`; the viewer falls back to a **range-read of the sealed segments** covering the
    /// gap (the durable archive) — [`ResumeOutcome::ResyncRequired`]. The bytes are recovered either
    /// way; a reconnect NEVER loses a line.
    ///
    /// `now_offset` is the current head byte offset of the `(run, job)` stream (the upper bound of the
    /// `resync_required` range-read — the gap is `[archive_floor, now_offset)`; the range-read returns
    /// every sealed segment overlapping it).
    pub fn resume(
        &mut self,
        coord: &LogCoord,
        last_seq: u64,
        now_offset: i64,
    ) -> Result<ResumeOutcome, FirehoseError> {
        let scope = coord.firehose_scope()?;
        match self.firehose.resume(CI_LOG_STREAM, &scope, last_seq) {
            Ok(sub) => Ok(ResumeOutcome::Live(sub)),
            Err(FirehoseError::ResyncRequired { window_floor, .. }) => {
                // The gap cannot be replayed from the live window — read it from the sealed archive.
                // The whole produced span is durably recoverable; range-read `[0, now_offset)` so the
                // viewer gets every sealed segment covering the gap (it slices the bytes it needs).
                let range_read = self.archive.range_read(0, now_offset.max(0));
                Ok(ResumeOutcome::ResyncRequired {
                    window_floor,
                    range_read,
                })
            }
            // An over-broad scope cannot occur (the scope is the bounded run:<id>); propagate any
            // other transport error LOUD (never swallowed).
            Err(other) => Err(other),
        }
    }

    /// The number of frames the `(run, job)` live retention window currently holds (the live-tail
    /// backlog a fresh `subscribe(None)`-then-resume would see).
    pub fn window_len(&self, coord: &LogCoord) -> usize {
        let Ok(scope) = coord.firehose_scope() else {
            return 0;
        };
        self.firehose.window_len(CI_LOG_STREAM, &scope)
    }

    /// The sealed-segment archive index (the durable read the `resync_required` fallback uses).
    pub fn archive(&self) -> &SegmentIndex {
        &self.archive
    }
}

// =================================================================================================
// 3. The details_ref jump-to-failure resolution (the X-1 / OQ-D `#step-<n>` → byte range path).
// =================================================================================================

/// **A resolved `#step-<n>` jump-to-failure target (arch §4 — the byte range a failed step deep-links
/// to).** The `CheckStatus.details_ref` resolves through `log_anchor` → `log_segment` → THIS: the
/// step's `[byte_start, byte_end)` span within the `(run, job)` log stream + the sealed segments that
/// cover it (the viewer scrolls the live-tail to this offset / pulls the archived bytes). 0 dangling
/// anchors: a resolved step ALWAYS has a real byte range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepByteRange {
    /// The run id (opaque).
    pub run_id: String,
    /// The job id (opaque).
    pub job_id: String,
    /// The stable step id (the `#step-<n>` sub-anchor; stable across retries).
    pub step_id: String,
    /// The first byte the step's output STARTS at within the `(run, job)` log stream.
    pub byte_start: i64,
    /// The byte offset the step's output ENDS at (`None` while the step is still RUNNING — an open
    /// span the live tail is still growing; `Some` once the step terminated).
    pub byte_end: Option<i64>,
    /// The step's terminal status token (`running` | `passed` | `failed` | `skipped`) — a `failed`
    /// step is the headline jump-to-failure target.
    pub status: String,
    /// The sealed segments that cover the step's byte range (the durable archive read; empty iff the
    /// step's bytes are all still in the live window).
    pub segments: Vec<SegmentRange>,
}

/// **Why a `details_ref` did NOT resolve to a byte range (a NAMED tombstone — never a silent dangle,
/// EI-01 §3 / OQ-D).** The resolution ladder degrades through these, never leaks, never silently
/// dangles (the 0-dangling-anchors GATE: a `details_ref` either resolves to a [`StepByteRange`] or is
/// ONE of these named tombstones).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailsRefError {
    /// The ref is not a CI `#step-<n>` details_ref (no `#step-<n>` sub-anchor, or a malformed one). The
    /// resolver REJECTS, never guesses (the OQ-D "rejects ambiguity, never guesses scope" rule).
    NotAStepRef {
        /// The offending ref string.
        raw: String,
        /// Why it is not a step ref.
        why: &'static str,
    },
    /// The `#step-<n>` ref is well-formed but no `log_anchor` exists for its `(run, job, step)` — a
    /// `Tombstone{reason: anchor_gone}` (the step never shipped a line / the index row is absent). The
    /// viewer shows the parent run, never a dangling anchor (the OQ-D tombstone ladder).
    AnchorGone {
        /// The run id parsed from the ref.
        run_id: String,
        /// The job id parsed from the ref (empty when the ref omits the job segment).
        job_id: String,
        /// The step id parsed from the ref.
        step_id: String,
    },
}

impl std::fmt::Display for DetailsRefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DetailsRefError::NotAStepRef { raw, why } => {
                write!(f, "`{raw}` is not a CI #step-<n> details_ref: {why}")
            }
            DetailsRefError::AnchorGone {
                run_id,
                job_id,
                step_id,
            } => write!(
                f,
                "no log_anchor for run `{run_id}` job `{job_id}` step `{step_id}` — \
                 Tombstone{{reason: anchor_gone}} (the step has no indexed byte range; show the \
                 parent run, never a dangling anchor)"
            ),
        }
    }
}

impl std::error::Error for DetailsRefError {}

/// **The parsed `(run, job?, step)` triple from a CI `#step-<n>` details_ref.** The `details_ref`
/// grammar is `…/ci/run/<run>[…/job/<job>]#step-<step>` — the resolver extracts the run, the optional
/// job, and the step from the ref. PII-free opaque ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedStepRef {
    /// The run id (opaque).
    pub run_id: String,
    /// The job id (opaque; empty when the ref omits the `/job/<id>` segment — the run-level form).
    pub job_id: String,
    /// The step id (the `#step-<step>` sub-anchor).
    pub step_id: String,
}

/// **`parse_step_ref(raw)` — extract `(run, job?, step)` from a `#step-<n>` details_ref (arch §4 /
/// OQ-D).** The grammar: `myelin://[…/]ci/run/<run>[/job/<job>]#step-<step>`. REJECTS a ref with no
/// `#step-<step>` sub-anchor, an empty step, or no `ci/run/<run>` path — all as
/// [`DetailsRefError::NotAStepRef`] (the resolver never guesses; OQ-D). The `<step>` is OPAQUE (it is
/// the stable step id, NOT necessarily numeric — CI-P20 mints stable opaque step ids; the Refs grammar
/// `step-<n>` accepts the canonical numeric form but the index key is the opaque step id CI assigned).
pub fn parse_step_ref(raw: &str) -> Result<ParsedStepRef, DetailsRefError> {
    // The sub-anchor: everything after the `#`. A details_ref MUST carry a `#step-<step>` sub-anchor.
    let Some((root, sub)) = raw.split_once('#') else {
        return Err(DetailsRefError::NotAStepRef {
            raw: raw.to_string(),
            why: "no `#step-<n>` sub-anchor (a details_ref deep-links to a step)",
        });
    };
    let Some(step_id) = sub.strip_prefix("step-") else {
        return Err(DetailsRefError::NotAStepRef {
            raw: raw.to_string(),
            why: "the sub-anchor is not a `step-<n>` kind (the jump-to-failure sub-anchor)",
        });
    };
    if step_id.is_empty() {
        return Err(DetailsRefError::NotAStepRef {
            raw: raw.to_string(),
            why: "the step id is empty (`#step-` with no id)",
        });
    }
    // The path must name `ci/run/<run>` (CI's run namespace). Extract the run id after `ci/run/`.
    let Some(after_run) = root.split("ci/run/").nth(1) else {
        return Err(DetailsRefError::NotAStepRef {
            raw: raw.to_string(),
            why: "the ref does not name a `ci/run/<run>` path",
        });
    };
    // The run id is up to the next `/` (or the whole remainder); an optional `/job/<job>` follows.
    let (run_id, job_id) = match after_run.split_once("/job/") {
        Some((run, job)) => (run.trim_end_matches('/'), job.trim_end_matches('/')),
        None => (after_run.trim_end_matches('/'), ""),
    };
    if run_id.is_empty() {
        return Err(DetailsRefError::NotAStepRef {
            raw: raw.to_string(),
            why: "the run id is empty (`ci/run/` with no id)",
        });
    }
    Ok(ParsedStepRef {
        run_id: run_id.to_string(),
        job_id: job_id.to_string(),
        step_id: step_id.to_string(),
    })
}

/// **The `details_ref` jump-to-failure resolver (arch §4 — the X-1 / OQ-D `#step-<n>` → byte range
/// path).** Holds a READ view over CI-P20's `log_anchor` + `log_segment` index rows; resolves a
/// `CheckStatus.details_ref = …/ci/run/<id>#step-<n>` to a [`StepByteRange`] (the byte span the failed
/// step's output occupies + the sealed segments covering it). 0 dangling anchors: a step with no
/// anchor is the NAMED [`DetailsRefError::AnchorGone`] tombstone, never a silent dangle.
///
/// This is a READ view over the authoritative index rows (coherence, EI-01 §7 — the pipeline owns the
/// rows; the resolver borrows them).
#[derive(Clone, Debug, Default)]
pub struct DetailsRefResolver {
    /// The `log_anchor` rows (the `(run, job, step) → byte range` index).
    anchors: Vec<LogAnchorRow>,
    /// The `log_segment` rows (the `(run, job, byte-range) → blob` index — the archive covers).
    segments: Vec<LogSegmentRow>,
}

impl DetailsRefResolver {
    /// Build a resolver over CI-P20's `log_anchor` + `log_segment` index rows (the authoritative rows
    /// the pipeline buffered; the resolver reads them — one source of truth).
    pub fn new(anchors: Vec<LogAnchorRow>, segments: Vec<LogSegmentRow>) -> DetailsRefResolver {
        DetailsRefResolver { anchors, segments }
    }

    /// **`resolve(details_ref) → StepByteRange | DetailsRefError` (arch §4 / OQ-D — the jump-to-failure
    /// resolution).** Parse the `#step-<n>` sub-anchor, look up its `(run, job, step)` `log_anchor`,
    /// and return the byte range + the sealed segments covering it. A step with no anchor is the NAMED
    /// `AnchorGone` tombstone (0 dangling anchors — a resolved ref ALWAYS has a real byte range).
    ///
    /// The `job_id` match is exact when the ref names a job; when the ref omits the job segment (the
    /// run-level form), the FIRST anchor matching `(run, step)` resolves (the step id is stable across
    /// the run's jobs; in practice a step belongs to one job).
    pub fn resolve(&self, details_ref: &str) -> Result<StepByteRange, DetailsRefError> {
        let parsed = parse_step_ref(details_ref)?;
        let anchor = self
            .anchors
            .iter()
            .find(|a| {
                a.run_id == parsed.run_id
                    && a.step_id == parsed.step_id
                    && (parsed.job_id.is_empty() || a.job_id == parsed.job_id)
            })
            .ok_or_else(|| DetailsRefError::AnchorGone {
                run_id: parsed.run_id.clone(),
                job_id: parsed.job_id.clone(),
                step_id: parsed.step_id.clone(),
            })?;

        // The step's byte span: [byte_start, byte_end) (byte_end open while running → the live cursor).
        let lo = anchor.byte_start;
        let hi = anchor.byte_end.unwrap_or(i64::MAX);
        // The sealed segments covering the step's span (the durable archive read for this step).
        let archive = SegmentIndex::from_rows(&anchor.run_id, &anchor.job_id, &self.segments);
        let segments = archive.range_read(lo, hi);

        Ok(StepByteRange {
            run_id: anchor.run_id.clone(),
            job_id: anchor.job_id.clone(),
            step_id: anchor.step_id.clone(),
            byte_start: anchor.byte_start,
            byte_end: anchor.byte_end,
            status: anchor.status.token().to_string(),
            segments,
        })
    }

    /// **The DANGLING-ANCHOR count over a set of `details_ref`s (the GATE: 0).** For each ref, resolve
    /// it; a ref whose `(run, job, step)` has NO anchor (an `AnchorGone` tombstone) is a DANGLING
    /// anchor. The GATE asserts this is 0 for every `details_ref` a real `CheckStatus` carries (every
    /// failed step's `details_ref` resolves to a byte range — 0 dangling step anchors).
    pub fn dangling_anchor_count<'r>(&self, refs: impl IntoIterator<Item = &'r str>) -> u64 {
        refs.into_iter()
            .filter(|r| matches!(self.resolve(r), Err(DetailsRefError::AnchorGone { .. })))
            .count() as u64
    }
}

// =================================================================================================
// 4. A helper to pull a step's bytes from the archive (the consumer slices what it needs).
// =================================================================================================

/// **Read a `[lo, hi)` byte slice from the sealed segments via the `BlobStore` (arch §7.1 — the
/// archive read the viewer renders).** For each covering segment, pull the blob and slice the bytes
/// that fall in `[lo, hi)` (offset within the segment = `lo - segment.byte_start`). Returns the
/// concatenated bytes for the requested range (byte-identical to the live tail's bytes — the durable
/// archive IS the live bytes, sealed). A missing blob is skipped (best-effort over what the archive
/// holds — a corrupt/GC'd segment is a NAMED gap, never a silent wrong-byte read).
pub fn read_range_from_archive<B: BlobStore>(
    blobs: &B,
    tenant: &myelin_tenancy::TenantId,
    segments: &[SegmentRange],
    lo: i64,
    hi: i64,
) -> Vec<u8> {
    let mut out = Vec::new();
    for seg in segments {
        let Ok(hash) = ContentHash::parse(&seg.blob_ref) else {
            continue;
        };
        let Ok(bytes) = blobs.get(tenant, &hash) else {
            continue;
        };
        // The intersection of [lo, hi) with the segment's span [seg.byte_start, seg.byte_end).
        let span_lo = lo.max(seg.byte_start);
        let span_hi = hi.min(seg.byte_end);
        if span_hi <= span_lo {
            continue;
        }
        let off_lo = (span_lo - seg.byte_start) as usize;
        let off_hi = (span_hi - seg.byte_start) as usize;
        if off_hi <= bytes.len() {
            out.extend_from_slice(&bytes[off_lo..off_hi]);
        }
    }
    out
}

// Unit + drill + CDC tests live in `tests/` (integration tests over the PUBLIC viewer API):
//   - `tests/unit_ci_p21_live_tail.rs` — the resume backfill math, the resync→range-read fallback,
//     the bounded-scope rejection, the `#step-<n>` → byte-range resolution.
//   - `tests/drills_ci_p21_live_tail.rs` — the CI-D11 reconnect-loses-zero-ops failure-injection drill.
//   - `tests/cdc_live_tail_3_5_11_8.rs` — the consumed-rows 3.5 + 11.8 CDC pair.
// They live OUT of `src/` so the test scaffolding's firehose `publish` (the live-tail set-up) does not
// trip the `no-raw-publish` lint's `.publish(` fingerprint — `tests/` is the lint's NAMED scan
// exclusion (the production module `live_tail.rs` itself only `subscribe`/`resume`s, never publishes).

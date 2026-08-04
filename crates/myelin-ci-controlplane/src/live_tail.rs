use crate::log_pipeline::CI_LOG_STREAM;
use crate::log_pipeline::{LogAnchorRow, LogCoord, LogSegmentRow};
use myelin_events::firehose::{Firehose, FirehoseError, Subscription};
use myelin_storage::{BlobStore, ContentHash};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentRange {
    pub blob_ref: String,
    pub byte_start: i64,
    pub byte_end: i64,
}

impl SegmentRange {
    fn overlaps(&self, lo: i64, hi: i64) -> bool {
        self.byte_start < hi && self.byte_end > lo
    }
}

#[derive(Clone, Debug, Default)]
pub struct SegmentIndex {
    segments: Vec<SegmentRange>,
}

impl SegmentIndex {
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

    pub fn range_read(&self, lo: i64, hi: i64) -> Vec<SegmentRange> {
        self.segments
            .iter()
            .filter(|s| s.overlaps(lo, hi))
            .cloned()
            .collect()
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

#[derive(Debug)]
pub enum ResumeOutcome {
    Live(Subscription),
    ResyncRequired {
        window_floor: u64,
        range_read: Vec<SegmentRange>,
    },
}

impl ResumeOutcome {
    pub fn is_live(&self) -> bool {
        matches!(self, ResumeOutcome::Live(_))
    }

    pub fn is_resync_required(&self) -> bool {
        matches!(self, ResumeOutcome::ResyncRequired { .. })
    }
}

pub struct LiveTail<'a> {
    firehose: &'a mut Firehose,
    archive: SegmentIndex,
}

impl<'a> LiveTail<'a> {
    pub fn new(firehose: &'a mut Firehose, archive: SegmentIndex) -> LiveTail<'a> {
        LiveTail { firehose, archive }
    }

    pub fn subscribe(
        &mut self,
        coord: &LogCoord,
        cursor: Option<u64>,
    ) -> Result<Subscription, FirehoseError> {
        let scope = coord.firehose_scope()?;
        self.firehose.subscribe(CI_LOG_STREAM, &scope, cursor)
    }

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
                let range_read = self.archive.range_read(0, now_offset.max(0));
                Ok(ResumeOutcome::ResyncRequired {
                    window_floor,
                    range_read,
                })
            }
            Err(other) => Err(other),
        }
    }

    pub fn window_len(&self, coord: &LogCoord) -> usize {
        let Ok(scope) = coord.firehose_scope() else {
            return 0;
        };
        self.firehose.window_len(CI_LOG_STREAM, &scope)
    }

    pub fn archive(&self) -> &SegmentIndex {
        &self.archive
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepByteRange {
    pub run_id: String,
    pub job_id: String,
    pub step_id: String,
    pub byte_start: i64,
    pub byte_end: Option<i64>,
    pub status: String,
    pub segments: Vec<SegmentRange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailsRefError {
    NotAStepRef {
        raw: String,
        why: &'static str,
    },
    AnchorGone {
        run_id: String,
        job_id: String,
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
                "no log_anchor for run `{run_id}` job `{job_id}` step `{step_id}` - \
                 Tombstone{{reason: anchor_gone}} (the step has no indexed byte range; show the \
                 parent run, never a dangling anchor)"
            ),
        }
    }
}

impl std::error::Error for DetailsRefError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedStepRef {
    pub run_id: String,
    pub job_id: String,
    pub step_id: String,
}

pub fn parse_step_ref(raw: &str) -> Result<ParsedStepRef, DetailsRefError> {
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
    let Some(after_run) = root.split("ci/run/").nth(1) else {
        return Err(DetailsRefError::NotAStepRef {
            raw: raw.to_string(),
            why: "the ref does not name a `ci/run/<run>` path",
        });
    };
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

#[derive(Clone, Debug, Default)]
pub struct DetailsRefResolver {
    anchors: Vec<LogAnchorRow>,
    segments: Vec<LogSegmentRow>,
}

impl DetailsRefResolver {
    pub fn new(anchors: Vec<LogAnchorRow>, segments: Vec<LogSegmentRow>) -> DetailsRefResolver {
        DetailsRefResolver { anchors, segments }
    }

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

        let lo = anchor.byte_start;
        let hi = anchor.byte_end.unwrap_or(i64::MAX);
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

    pub fn dangling_anchor_count<'r>(&self, refs: impl IntoIterator<Item = &'r str>) -> u64 {
        refs.into_iter()
            .filter(|r| matches!(self.resolve(r), Err(DetailsRefError::AnchorGone { .. })))
            .count() as u64
    }
}

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

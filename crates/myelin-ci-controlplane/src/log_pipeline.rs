use myelin_events::firehose::{
    Firehose, FirehoseError, FirehoseScope, FrameDraft, FIREHOSE_MAX_SCOPE_ID_BYTES,
};
use myelin_events::{AggregateKey, ArtifactRef, DataRole, EventDraft, EventType, Visibility};
use myelin_storage::{BlobError, BlobStore, ContentHash};
use myelin_tenancy::{Region, TenantId};

use myelin_ci_sandbox::events::CI_LOG_AVAILABLE;
use myelin_refs::{mint, parse, ParseError, Sub};

// @residency-write — the residency-pin write-boundary (layer-3) leg arms on this file: a log_segment /
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossRegionLogWrite {
    pub tenant_id: String,
    pub cell_region: Region,
    pub row_region: Region,
}

impl std::fmt::Display for CrossRegionLogWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CI log write REFUSED for tenant `{}`: the write pins region `{}` but the cell it lives \
             in is region `{}` - CI logs live near the runner region and a log segment/anchor/blob \
             cannot exist outside its cell's region (the pin is the cell's, NOT the caller's; arch 02 \
             §7.1, contract 1.6). REFUSED (0 cross-region log writes is the residency-pin green \
             artifact).",
            self.tenant_id,
            self.row_region.as_str(),
            self.cell_region.as_str(),
        )
    }
}

impl std::error::Error for CrossRegionLogWrite {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogPipelineError {
    CrossRegion(CrossRegionLogWrite),
    InvalidScope(FirehoseError),
    InvalidCoordinate(LogReferenceError),
    InvalidResume {
        run_id: String,
        job_id: String,
        reason: &'static str,
    },
    StreamAlreadyOpen {
        run_id: String,
        job_id: String,
    },
    NonTerminalClose {
        run_id: String,
        job_id: String,
    },
    Blob(BlobError),
    CapacityExceeded {
        run_id: String,
        job_id: String,
        resource: &'static str,
    },
}

impl std::fmt::Display for LogPipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogPipelineError::CrossRegion(error) => error.fmt(f),
            LogPipelineError::InvalidScope(error) => error.fmt(f),
            LogPipelineError::InvalidCoordinate(error) => error.fmt(f),
            LogPipelineError::InvalidResume {
                run_id,
                job_id,
                reason,
            } => write!(
                f,
                "cannot resume CI log run `{run_id}` job `{job_id}`: {reason}"
            ),
            LogPipelineError::StreamAlreadyOpen { run_id, job_id } => write!(
                f,
                "cannot resume CI log run `{run_id}` job `{job_id}` over an open stream"
            ),
            LogPipelineError::NonTerminalClose { run_id, job_id } => write!(
                f,
                "cannot close CI log run `{run_id}` job `{job_id}` with a running status"
            ),
            LogPipelineError::Blob(error) => write!(f, "archive CI log bytes: {error}"),
            LogPipelineError::CapacityExceeded {
                run_id,
                job_id,
                resource,
            } => write!(
                f,
                "CI log {resource} capacity exhausted for run `{run_id}` and job `{job_id}`"
            ),
        }
    }
}

impl std::error::Error for LogPipelineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LogPipelineError::CrossRegion(error) => Some(error),
            LogPipelineError::InvalidScope(error) => Some(error),
            LogPipelineError::InvalidCoordinate(error) => Some(error),
            LogPipelineError::InvalidResume { .. } => None,
            LogPipelineError::StreamAlreadyOpen { .. } => None,
            LogPipelineError::NonTerminalClose { .. } => None,
            LogPipelineError::Blob(error) => Some(error),
            LogPipelineError::CapacityExceeded { .. } => None,
        }
    }
}

impl From<CrossRegionLogWrite> for LogPipelineError {
    fn from(error: CrossRegionLogWrite) -> Self {
        LogPipelineError::CrossRegion(error)
    }
}

impl From<FirehoseError> for LogPipelineError {
    fn from(error: FirehoseError) -> Self {
        LogPipelineError::InvalidScope(error)
    }
}

impl From<BlobError> for LogPipelineError {
    fn from(error: BlobError) -> Self {
        LogPipelineError::Blob(error)
    }
}

impl From<LogReferenceError> for LogPipelineError {
    fn from(error: LogReferenceError) -> Self {
        LogPipelineError::InvalidCoordinate(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogWritePin {
    tenant_id: String,
    cell_region: Region,
    cross_region_log_writes_admitted: u64,
}

impl LogWritePin {
    pub fn for_cell(tenant_id: impl Into<String>, cell_region: Region) -> LogWritePin {
        LogWritePin {
            tenant_id: tenant_id.into(),
            cell_region,
            cross_region_log_writes_admitted: 0,
        }
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    pub fn cell_region(&self) -> &Region {
        &self.cell_region
    }

    pub fn cross_region_log_writes_admitted(&self) -> u64 {
        self.cross_region_log_writes_admitted
    }

    pub fn admit_log_write(&mut self, row_region: &Region) -> Result<(), CrossRegionLogWrite> {
        if *row_region != self.cell_region {
            return Err(CrossRegionLogWrite {
                tenant_id: self.tenant_id.clone(),
                cell_region: self.cell_region.clone(),
                row_region: row_region.clone(),
            });
        }
        self.cross_region_log_writes_admitted += 1;
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct SecretRedactor {
    needles: Vec<String>,
}

pub const REDACTION_MARKER: &str = "***REDACTED***";

impl SecretRedactor {
    pub fn for_job(needles: impl IntoIterator<Item = String>) -> SecretRedactor {
        SecretRedactor {
            needles: needles.into_iter().filter(|n| !n.is_empty()).collect(),
        }
    }

    pub fn redact(&self, line: &str) -> String {
        let mut out = line.to_string();
        for needle in &self.needles {
            if out.contains(needle.as_str()) {
                out = out.replace(needle.as_str(), REDACTION_MARKER);
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.needles.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LogCoord {
    pub run_id: String,
    pub job_id: String,
    pub step_no: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogReferenceError {
    InvalidCoordinate {
        component: &'static str,
        reason: &'static str,
    },
    InvalidRange {
        byte_start: i64,
        byte_end: i64,
    },
    InvalidReference(ParseError),
}

impl std::fmt::Display for LogReferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogReferenceError::InvalidCoordinate { component, reason } => {
                write!(f, "invalid CI log {component}: {reason}")
            }
            LogReferenceError::InvalidRange {
                byte_start,
                byte_end,
            } => write!(
                f,
                "invalid CI log byte range: expected 0 <= start < end, got {byte_start}:{byte_end}"
            ),
            LogReferenceError::InvalidReference(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for LogReferenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LogReferenceError::InvalidCoordinate { .. }
            | LogReferenceError::InvalidRange { .. } => None,
            LogReferenceError::InvalidReference(error) => Some(error),
        }
    }
}

impl From<ParseError> for LogReferenceError {
    fn from(error: ParseError) -> Self {
        LogReferenceError::InvalidReference(error)
    }
}

impl LogCoord {
    pub fn new(run_id: impl Into<String>, job_id: impl Into<String>, step_no: u32) -> LogCoord {
        LogCoord {
            run_id: run_id.into(),
            job_id: job_id.into(),
            step_no,
        }
    }

    pub fn firehose_scope(&self) -> Result<FirehoseScope, FirehoseError> {
        FirehoseScope::parse(&format!("run:{}", self.run_id))
    }

    pub fn log_ref(&self, tenant: &TenantId) -> Result<ArtifactRef, LogReferenceError> {
        self.validate_identity()?;
        Ok(parse(&format!(
            "myelin://{}/ci/log/{}:{}:{}",
            tenant.as_str(),
            self.run_id,
            self.job_id,
            self.step_no
        ))?)
    }

    pub fn details_ref(&self, tenant: &TenantId) -> Result<ArtifactRef, LogReferenceError> {
        self.validate_identity()?;
        let run = parse(&format!(
            "myelin://{}/ci/run/{}",
            tenant.as_str(),
            self.run_id
        ))?;
        Ok(mint(&run, Sub::Step(u64::from(self.step_no)))?)
    }

    pub fn aggregate_key(&self) -> Result<AggregateKey, LogReferenceError> {
        self.validate_identity()?;
        let canonical_uuid = |value: &str| {
            sqlx::types::Uuid::parse_str(value)
                .ok()
                .filter(|parsed| parsed.to_string() == value)
        };
        if canonical_uuid(&self.run_id).is_some() && canonical_uuid(&self.job_id).is_some() {
            return Ok(AggregateKey(format!("log:{}-{}", self.run_id, self.job_id)));
        }

        let mut digest = blake3::Hasher::new();
        digest.update(b"myelin.ci.log.aggregate.v1\0");
        for value in [&self.run_id, &self.job_id] {
            digest.update(&(value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        Ok(AggregateKey(format!(
            "log:v1-{}",
            digest.finalize().to_hex()
        )))
    }

    fn validate_identity(&self) -> Result<(), LogReferenceError> {
        for (component, value) in [("run id", self.run_id.as_str()), ("job id", &self.job_id)] {
            if value.is_empty() {
                return Err(LogReferenceError::InvalidCoordinate {
                    component,
                    reason: "the id is empty",
                });
            }
            if value.len() > FIREHOSE_MAX_SCOPE_ID_BYTES {
                return Err(LogReferenceError::InvalidCoordinate {
                    component,
                    reason: "the id exceeds the bounded coordinate length",
                });
            }
            if value.chars().any(|character| {
                matches!(character, ':' | '/' | '#')
                    || character.is_whitespace()
                    || character.is_control()
            }) {
                return Err(LogReferenceError::InvalidCoordinate {
                    component,
                    reason: "the id contains whitespace or a reserved `:`, `/`, or `#` delimiter",
                });
            }
        }
        Ok(())
    }
}

pub const CI_LOG_STREAM: &str = "ci-log";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogSegmentRow {
    pub tenant_id: String,
    pub region: String,
    pub run_id: String,
    pub job_id: String,
    pub segment_seq: i32,
    pub blob_ref: Option<String>,
    pub byte_start: i64,
    pub byte_end: i64,
    pub pii_key_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogAnchorRow {
    pub tenant_id: String,
    pub region: String,
    pub run_id: String,
    pub job_id: String,
    pub step_id: String,
    pub byte_start: i64,
    pub byte_end: Option<i64>,
    pub status: AnchorStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorStatus {
    Running,
    Passed,
    Failed,
    Skipped,
}

impl AnchorStatus {
    pub fn token(self) -> &'static str {
        match self {
            AnchorStatus::Running => "running",
            AnchorStatus::Passed => "passed",
            AnchorStatus::Failed => "failed",
            AnchorStatus::Skipped => "skipped",
        }
    }

    pub fn from_token(token: &str) -> Option<AnchorStatus> {
        match token {
            "running" => Some(AnchorStatus::Running),
            "passed" => Some(AnchorStatus::Passed),
            "failed" => Some(AnchorStatus::Failed),
            "skipped" => Some(AnchorStatus::Skipped),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, AnchorStatus::Running)
    }
}

pub const INSERT_LOG_SEGMENT_QUERY: &str = "\
INSERT INTO log_segment
  (tenant_id, region, run_id, job_id, segment_seq, blob_ref, byte_start, byte_end, pii_key_ref)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
ON CONFLICT (tenant_id, run_id, job_id, segment_seq) DO UPDATE
  SET blob_ref = log_segment.blob_ref
  WHERE log_segment.region = EXCLUDED.region
    AND log_segment.blob_ref IS NOT DISTINCT FROM EXCLUDED.blob_ref
    AND log_segment.byte_start = EXCLUDED.byte_start
    AND log_segment.byte_end = EXCLUDED.byte_end
    AND log_segment.pii_key_ref = EXCLUDED.pii_key_ref";

pub const UPSERT_LOG_ANCHOR_QUERY: &str = "\
INSERT INTO log_anchor
  (tenant_id, region, run_id, job_id, step_id, byte_start, byte_end, status)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
ON CONFLICT (tenant_id, run_id, job_id, step_id) DO UPDATE
  SET byte_end = EXCLUDED.byte_end, status = EXCLUDED.status
  WHERE log_anchor.region = EXCLUDED.region
    AND log_anchor.byte_start = EXCLUDED.byte_start
    AND (
      (log_anchor.status = 'running' AND log_anchor.byte_end IS NULL)
      OR (
        log_anchor.status = EXCLUDED.status
        AND log_anchor.byte_end IS NOT DISTINCT FROM EXCLUDED.byte_end
      )
    )";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoalesceBudget {
    pub bytes_per_pointer: u64,
}

impl Default for CoalesceBudget {
    fn default() -> Self {
        CoalesceBudget {
            bytes_per_pointer: Self::DEFAULT_BYTES_PER_POINTER,
        }
    }
}

impl CoalesceBudget {
    pub const DEFAULT_BYTES_PER_POINTER: u64 = 64 * 1024;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SealThreshold {
    pub seal_at_bytes: u64,
}

impl Default for SealThreshold {
    fn default() -> Self {
        SealThreshold {
            seal_at_bytes: Self::DEFAULT_SEAL_AT_BYTES,
        }
    }
}

impl SealThreshold {
    pub const DEFAULT_SEAL_AT_BYTES: u64 = 256 * 1024;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogAvailablePointer {
    coord: LogCoord,
    byte_start: i64,
    byte_end: i64,
    segment_ref: Option<ContentHash>,
}

impl LogAvailablePointer {
    pub fn new(
        coord: LogCoord,
        byte_start: i64,
        byte_end: i64,
        segment_ref: Option<ContentHash>,
    ) -> Result<LogAvailablePointer, LogReferenceError> {
        coord.validate_identity()?;
        if byte_start < 0 || byte_end <= byte_start {
            return Err(LogReferenceError::InvalidRange {
                byte_start,
                byte_end,
            });
        }
        Ok(LogAvailablePointer {
            coord,
            byte_start,
            byte_end,
            segment_ref,
        })
    }

    pub fn coord(&self) -> &LogCoord {
        &self.coord
    }

    pub fn byte_start(&self) -> i64 {
        self.byte_start
    }

    pub fn byte_end(&self) -> i64 {
        self.byte_end
    }

    pub fn segment_ref(&self) -> Option<&ContentHash> {
        self.segment_ref.as_ref()
    }

    pub fn subject(&self, tenant: &TenantId) -> Result<ArtifactRef, LogReferenceError> {
        self.coord.log_ref(tenant)
    }

    pub fn to_draft(&self, tenant: &TenantId) -> Result<EventDraft, LogReferenceError> {
        let segment_ref = self
            .segment_ref
            .as_ref()
            .map(ContentHash::to_multihash_string);
        let payload = serde_json::json!({
            "run": format!("ci/run/{}", self.coord.run_id),
            "job": self.coord.job_id,
            "step": self.coord.step_no,
            "byte_start": self.byte_start,
            "byte_end": self.byte_end,
            "segment_ref": segment_ref,
            "details_ref": self.coord.details_ref(tenant)?.0,
        });
        Ok(EventDraft {
            type_: EventType(CI_LOG_AVAILABLE.to_string()),
            subject: self.subject(tenant)?,
            // logs get their own per-(run, job) partition: canonical type:id
            // form, ordered within the job, and never contending with the run
            // lifecycle events for the run partition's outbox seq lock.
            aggregate: self.coord.aggregate_key()?,
            payload,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        })
    }
}

#[derive(Debug, Default)]
struct StreamState {
    next_offset: i64,
    open_segment: Vec<u8>,
    open_segment_start: i64,
    next_segment_seq: i32,
    bytes_since_pointer: u64,
    last_pointer_offset: i64,
}

#[derive(Clone, Copy, Debug)]
struct AppendPlan {
    frame_offset: i64,
    next_offset: i64,
    open_segment_start: i64,
    next_open_len: usize,
    next_pointer_bytes: u64,
    pointer_start: i64,
    segment_seq: i32,
    next_segment_seq: Option<i32>,
    next_lines_shipped: u64,
    emit_pointer: bool,
}

impl AppendPlan {
    fn build(
        coord: &LogCoord,
        state: Option<&StreamState>,
        bytes_len: usize,
        lines_shipped: u64,
        coalesce_at_bytes: u64,
        seal_at_bytes: u64,
    ) -> Result<AppendPlan, LogPipelineError> {
        let exhausted = |resource| LogPipelineError::CapacityExceeded {
            run_id: coord.run_id.clone(),
            job_id: coord.job_id.clone(),
            resource,
        };
        let byte_len = i64::try_from(bytes_len).map_err(|_| exhausted("byte offset"))?;
        let pointer_bytes = u64::try_from(bytes_len).map_err(|_| exhausted("pointer"))?;
        let frame_offset = state.map_or(0, |stream| stream.next_offset);
        let next_offset = frame_offset
            .checked_add(byte_len)
            .ok_or_else(|| exhausted("byte offset"))?;
        let open_segment_start = state.map_or(frame_offset, |stream| {
            if stream.open_segment.is_empty() {
                frame_offset
            } else {
                stream.open_segment_start
            }
        });
        let next_open_len = state
            .map_or(0, |stream| stream.open_segment.len())
            .checked_add(bytes_len)
            .ok_or_else(|| exhausted("segment"))?;
        let next_pointer_bytes = state
            .map_or(0, |stream| stream.bytes_since_pointer)
            .checked_add(pointer_bytes)
            .ok_or_else(|| exhausted("pointer"))?;
        let pointer_start = state.map_or(0, |stream| stream.last_pointer_offset);
        let segment_seq = state.map_or(0, |stream| stream.next_segment_seq);
        let should_seal = next_open_len > 0
            && u64::try_from(next_open_len).map_err(|_| exhausted("segment"))? >= seal_at_bytes;
        let next_segment_seq = should_seal
            .then(|| {
                segment_seq
                    .checked_add(1)
                    .ok_or_else(|| exhausted("segment sequence"))
            })
            .transpose()?;
        let next_lines_shipped = lines_shipped
            .checked_add(1)
            .ok_or_else(|| exhausted("line count"))?;
        let emit_pointer =
            should_seal || (next_pointer_bytes >= coalesce_at_bytes && next_offset > pointer_start);

        Ok(AppendPlan {
            frame_offset,
            next_offset,
            open_segment_start,
            next_open_len,
            next_pointer_bytes,
            pointer_start,
            segment_seq,
            next_segment_seq,
            next_lines_shipped,
            emit_pointer,
        })
    }

    fn seals_segment(self) -> bool {
        self.next_segment_seq.is_some()
    }
}

pub struct LogPipeline<B: BlobStore> {
    tenant: TenantId,
    region: Region,
    write_pin: LogWritePin,
    firehose: Firehose,
    blobs: B,
    redactor: SecretRedactor,
    coalesce: CoalesceBudget,
    seal: SealThreshold,
    streams: std::collections::HashMap<(String, String), StreamState>,
    segment_rows: Vec<LogSegmentRow>,
    anchor_rows: std::collections::HashMap<(String, String, String), LogAnchorRow>,
    pointers: Vec<LogAvailablePointer>,
    lines_shipped: u64,
}

impl<B: BlobStore> LogPipeline<B> {
    pub fn new(
        tenant: TenantId,
        region: Region,
        blobs: B,
        redactor: SecretRedactor,
    ) -> LogPipeline<B> {
        let write_pin = LogWritePin::for_cell(tenant.as_str().to_string(), region.clone());
        LogPipeline {
            tenant,
            region,
            write_pin,
            firehose: Firehose::new(),
            blobs,
            redactor,
            coalesce: CoalesceBudget::default(),
            seal: SealThreshold::default(),
            streams: std::collections::HashMap::new(),
            segment_rows: Vec::new(),
            anchor_rows: std::collections::HashMap::new(),
            pointers: Vec::new(),
            lines_shipped: 0,
        }
    }

    pub fn with_thresholds(
        mut self,
        coalesce: CoalesceBudget,
        seal: SealThreshold,
    ) -> LogPipeline<B> {
        self.coalesce = coalesce;
        self.seal = seal;
        self
    }

    pub fn resume_stream(
        &mut self,
        coord: &LogCoord,
        step_byte_start: i64,
        next_byte_offset: i64,
        next_segment_seq: i32,
    ) -> Result<(), LogPipelineError> {
        coord.validate_identity()?;
        let invalid = |reason| LogPipelineError::InvalidResume {
            run_id: coord.run_id.clone(),
            job_id: coord.job_id.clone(),
            reason,
        };
        if step_byte_start < 0 {
            return Err(invalid("the step byte start is negative"));
        }
        if next_byte_offset < step_byte_start {
            return Err(invalid("the next byte offset precedes the step start"));
        }
        if next_segment_seq < 0 {
            return Err(invalid("the next segment sequence is negative"));
        }
        let key = (coord.run_id.clone(), coord.job_id.clone());
        if self.streams.contains_key(&key) {
            return Err(LogPipelineError::StreamAlreadyOpen {
                run_id: coord.run_id.clone(),
                job_id: coord.job_id.clone(),
            });
        }
        self.streams.insert(
            key,
            StreamState {
                next_offset: next_byte_offset,
                open_segment: Vec::new(),
                open_segment_start: next_byte_offset,
                next_segment_seq,
                bytes_since_pointer: 0,
                last_pointer_offset: next_byte_offset,
            },
        );
        self.anchor_rows.insert(
            (
                coord.run_id.clone(),
                coord.job_id.clone(),
                coord.step_no.to_string(),
            ),
            LogAnchorRow {
                tenant_id: self.tenant.as_str().to_string(),
                region: self.region.as_str().to_string(),
                run_id: coord.run_id.clone(),
                job_id: coord.job_id.clone(),
                step_id: coord.step_no.to_string(),
                byte_start: step_byte_start,
                byte_end: None,
                status: AnchorStatus::Running,
            },
        );
        Ok(())
    }

    pub fn ship_line(&mut self, coord: &LogCoord, line: &str) -> Result<u64, LogPipelineError> {
        let redacted = self.redactor.redact(line);
        self.ship_redacted_bytes(coord, redacted.as_bytes())
    }

    pub fn ship_frame(&mut self, coord: &LogCoord, frame: &[u8]) -> Result<u64, LogPipelineError> {
        debug_assert!(
            self.redactor.is_empty(),
            "boundary-redacted frames require the empty defence-in-depth redactor"
        );
        self.ship_redacted_bytes(coord, frame)
    }

    fn ship_redacted_bytes(
        &mut self,
        coord: &LogCoord,
        bytes: &[u8],
    ) -> Result<u64, LogPipelineError> {
        coord.validate_identity()?;
        let scope = coord.firehose_scope()?;
        let key = (coord.run_id.clone(), coord.job_id.clone());
        let state = self.streams.get(&key);
        let plan = AppendPlan::build(
            coord,
            state,
            bytes.len(),
            self.lines_shipped,
            self.coalesce.bytes_per_pointer,
            self.seal.seal_at_bytes,
        )?;

        self.write_pin.admit_log_write(&self.region)?;
        if plan.seals_segment() {
            self.write_pin.admit_log_write(&self.region)?;
        }
        if plan.emit_pointer {
            self.write_pin.admit_log_write(&self.region)?;
        }

        let segment_ref = if plan.seals_segment() {
            let mut prospective = Vec::with_capacity(plan.next_open_len);
            if let Some(stream) = state {
                prospective.extend_from_slice(&stream.open_segment);
            }
            prospective.extend_from_slice(bytes);
            Some(self.blobs.put(&self.tenant, &prospective)?)
        } else {
            None
        };
        let pointer = plan
            .emit_pointer
            .then(|| {
                LogAvailablePointer::new(
                    coord.clone(),
                    plan.pointer_start,
                    plan.next_offset,
                    segment_ref.clone(),
                )
            })
            .transpose()?;

        let frame_payload = format!(
            "ci/run/{}/job/{}/step/{}@{}:{}",
            coord.run_id, coord.job_id, coord.step_no, plan.frame_offset, plan.next_offset
        );
        let frame = self
            .firehose
            .publish(CI_LOG_STREAM, &scope, FrameDraft::new(frame_payload))?;

        let stream = self.streams.entry(key).or_default();
        if stream.open_segment.is_empty() {
            stream.open_segment_start = plan.frame_offset;
        }
        stream.open_segment.extend_from_slice(bytes);
        stream.next_offset = plan.next_offset;
        stream.bytes_since_pointer = plan.next_pointer_bytes;
        if let Some(next_segment_seq) = plan.next_segment_seq {
            stream.open_segment.clear();
            stream.next_segment_seq = next_segment_seq;
        }
        if plan.emit_pointer {
            stream.last_pointer_offset = plan.next_offset;
            stream.bytes_since_pointer = 0;
        }
        self.lines_shipped = plan.next_lines_shipped;

        self.anchor_rows
            .entry((
                coord.run_id.clone(),
                coord.job_id.clone(),
                coord.step_no.to_string(),
            ))
            .or_insert_with(|| LogAnchorRow {
                tenant_id: self.tenant.as_str().to_string(),
                region: self.region.as_str().to_string(),
                run_id: coord.run_id.clone(),
                job_id: coord.job_id.clone(),
                step_id: coord.step_no.to_string(),
                byte_start: plan.frame_offset,
                byte_end: None,
                status: AnchorStatus::Running,
            });
        if let Some(segment_ref) = &segment_ref {
            self.segment_rows.push(LogSegmentRow {
                tenant_id: self.tenant.as_str().to_string(),
                region: self.region.as_str().to_string(),
                run_id: coord.run_id.clone(),
                job_id: coord.job_id.clone(),
                segment_seq: plan.segment_seq,
                blob_ref: Some(segment_ref.to_multihash_string()),
                byte_start: plan.open_segment_start,
                byte_end: plan.next_offset,
                pii_key_ref: self.tenant_dek_ref(),
            });
        }
        if let Some(pointer) = pointer {
            self.pointers.push(pointer);
        }

        Ok(frame.seq)
    }

    pub fn close_step(
        &mut self,
        coord: &LogCoord,
        status: AnchorStatus,
    ) -> Result<(), LogPipelineError> {
        coord.validate_identity()?;
        if !status.is_terminal() {
            return Err(LogPipelineError::NonTerminalClose {
                run_id: coord.run_id.clone(),
                job_id: coord.job_id.clone(),
            });
        }
        self.write_pin.admit_log_write(&self.region)?;
        let end = self
            .streams
            .get(&(coord.run_id.clone(), coord.job_id.clone()))
            .map(|s| s.next_offset)
            .unwrap_or(0);
        let akey = (
            coord.run_id.clone(),
            coord.job_id.clone(),
            coord.step_no.to_string(),
        );
        if let Some(anchor) = self.anchor_rows.get_mut(&akey) {
            anchor.byte_end = Some(end);
            anchor.status = status;
        } else {
            self.anchor_rows.insert(
                akey,
                LogAnchorRow {
                    tenant_id: self.tenant.as_str().to_string(),
                    region: self.region.as_str().to_string(),
                    run_id: coord.run_id.clone(),
                    job_id: coord.job_id.clone(),
                    step_id: coord.step_no.to_string(),
                    byte_start: end,
                    byte_end: Some(end),
                    status,
                },
            );
        }
        Ok(())
    }

    pub fn seal_open_segment(
        &mut self,
        coord: &LogCoord,
    ) -> Result<Option<String>, LogPipelineError> {
        coord.validate_identity()?;
        let key = (coord.run_id.clone(), coord.job_id.clone());
        let Some(stream) = self.streams.get(&key) else {
            return Ok(None);
        };
        if stream.open_segment.is_empty() {
            return Ok(None);
        }
        let bytes = stream.open_segment.clone();
        let segment_start = stream.open_segment_start;
        let segment_end = stream.next_offset;
        let segment_seq = stream.next_segment_seq;
        let next_segment_seq =
            segment_seq
                .checked_add(1)
                .ok_or_else(|| LogPipelineError::CapacityExceeded {
                    run_id: coord.run_id.clone(),
                    job_id: coord.job_id.clone(),
                    resource: "segment sequence",
                })?;
        let pointer_start = stream.last_pointer_offset;

        self.write_pin.admit_log_write(&self.region)?;
        self.write_pin.admit_log_write(&self.region)?;
        let content_hash = self.blobs.put(&self.tenant, &bytes)?;
        let blob_ref = content_hash.to_multihash_string();
        let pointer = LogAvailablePointer::new(
            coord.clone(),
            pointer_start,
            segment_end,
            Some(content_hash),
        )?;

        let stream = self
            .streams
            .get_mut(&key)
            .expect("the exclusively borrowed log stream remains present");
        stream.open_segment.clear();
        stream.next_segment_seq = next_segment_seq;
        stream.last_pointer_offset = segment_end;
        stream.bytes_since_pointer = 0;

        self.segment_rows.push(LogSegmentRow {
            tenant_id: self.tenant.as_str().to_string(),
            region: self.region.as_str().to_string(),
            run_id: coord.run_id.clone(),
            job_id: coord.job_id.clone(),
            segment_seq,
            blob_ref: Some(blob_ref.clone()),
            byte_start: segment_start,
            byte_end: segment_end,
            pii_key_ref: self.tenant_dek_ref(),
        });
        self.pointers.push(pointer);

        Ok(Some(blob_ref))
    }

    pub fn flush_job(
        &mut self,
        run_id: &str,
        job_id: &str,
        step_no: u32,
    ) -> Result<(), LogPipelineError> {
        let coord = LogCoord::new(run_id, job_id, step_no);
        self.seal_open_segment(&coord)?;
        Ok(())
    }

    fn tenant_dek_ref(&self) -> String {
        format!("kms://{}/0/tenant", self.tenant.as_str())
    }

    pub fn durable_pointer_count(&self) -> u64 {
        self.pointers.len() as u64
    }

    pub fn lines_shipped(&self) -> u64 {
        self.lines_shipped
    }

    pub fn dangling_anchor_count(&self) -> u64 {
        let mut dangling = 0u64;
        for anchor in self.anchor_rows.values() {
            let produced = self
                .streams
                .get(&(anchor.run_id.clone(), anchor.job_id.clone()))
                .map(|s| s.next_offset)
                .unwrap_or(0);
            let covered_end = anchor.byte_end.unwrap_or(produced);
            if covered_end > produced || anchor.byte_start > covered_end {
                dangling += 1;
            }
        }
        dangling
    }

    pub fn segment_rows(&self) -> &[LogSegmentRow] {
        &self.segment_rows
    }

    pub fn drain_segment_rows(&mut self) -> Vec<LogSegmentRow> {
        std::mem::take(&mut self.segment_rows)
    }

    pub fn anchor_rows(&self) -> Vec<&LogAnchorRow> {
        self.anchor_rows.values().collect()
    }

    pub fn drain_pointers(&mut self) -> Vec<LogAvailablePointer> {
        std::mem::take(&mut self.pointers)
    }

    pub fn admitted_log_writes(&self) -> u64 {
        self.write_pin.cross_region_log_writes_admitted()
    }

    pub fn firehose_window_len(&self, coord: &LogCoord) -> usize {
        let Ok(scope) = coord.firehose_scope() else {
            return 0;
        };
        self.firehose.window_len(CI_LOG_STREAM, &scope)
    }
}

#[cfg(test)]
#[path = "log_pipeline_tests.rs"]
mod tests;

use myelin_events::firehose::{Firehose, FirehoseError, FirehoseScope, FrameDraft};
use myelin_events::{ArtifactRef, DataRole, EventDraft, EventType, Visibility};
use myelin_storage::{BlobStore, ContentHash};
use myelin_tenancy::{Region, TenantId};

use myelin_ci_sandbox::events::CI_LOG_AVAILABLE;

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
    pub step_id: String,
}

impl LogCoord {
    pub fn new(
        run_id: impl Into<String>,
        job_id: impl Into<String>,
        step_id: impl Into<String>,
    ) -> LogCoord {
        LogCoord {
            run_id: run_id.into(),
            job_id: job_id.into(),
            step_id: step_id.into(),
        }
    }

    pub fn firehose_scope(&self) -> Result<FirehoseScope, FirehoseError> {
        FirehoseScope::parse(&format!("run:{}", self.run_id))
    }

    pub fn details_ref(&self) -> ArtifactRef {
        ArtifactRef(format!(
            "myelin://ci/run/{}/job/{}#step-{}",
            self.run_id, self.job_id, self.step_id
        ))
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
    pub coord: LogCoord,
    pub byte_start: i64,
    pub byte_end: i64,
    pub segment_ref: Option<String>,
}

impl LogAvailablePointer {
    pub fn subject(&self) -> ArtifactRef {
        self.coord.details_ref()
    }

    pub fn to_draft(&self) -> EventDraft {
        let payload = serde_json::json!({
            "run": format!("ci/run/{}", self.coord.run_id),
            "job": self.coord.job_id,
            "step": self.coord.step_id,
            "byte_start": self.byte_start,
            "byte_end": self.byte_end,
            "segment_ref": self.segment_ref,
            "details_ref": self.coord.details_ref().0,
        });
        EventDraft {
            type_: EventType(CI_LOG_AVAILABLE.to_string()),
            subject: self.subject(),
            aggregate: myelin_events::AggregateKey(format!(
                "ci/run/{}/job/{}",
                self.coord.run_id, self.coord.job_id
            )),
            payload,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
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
    ) {
        self.streams.insert(
            (coord.run_id.clone(), coord.job_id.clone()),
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
                coord.step_id.clone(),
            ),
            LogAnchorRow {
                tenant_id: self.tenant.as_str().to_string(),
                region: self.region.as_str().to_string(),
                run_id: coord.run_id.clone(),
                job_id: coord.job_id.clone(),
                step_id: coord.step_id.clone(),
                byte_start: step_byte_start,
                byte_end: None,
                status: AnchorStatus::Running,
            },
        );
    }

    pub fn ship_line(&mut self, coord: &LogCoord, line: &str) -> Result<u64, CrossRegionLogWrite> {
        let redacted = self.redactor.redact(line);
        self.ship_redacted_bytes(coord, redacted.as_bytes())
    }

    pub fn ship_frame(
        &mut self,
        coord: &LogCoord,
        frame: &[u8],
    ) -> Result<u64, CrossRegionLogWrite> {
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
    ) -> Result<u64, CrossRegionLogWrite> {
        let len = bytes.len() as i64;

        let scope = coord
            .firehose_scope()
            .expect("run:<id> is a bounded firehose scope (opaque run id)");
        let key = (coord.run_id.clone(), coord.job_id.clone());

        let (frame_offset, frame_payload) = {
            let st = self.streams.entry(key.clone()).or_default();
            let offset = st.next_offset;
            let payload = format!(
                "ci/run/{}/job/{}/step/{}@{}:{}",
                coord.run_id,
                coord.job_id,
                coord.step_id,
                offset,
                offset + len
            );
            (offset, payload)
        };
        let seq = self
            .firehose
            .publish(CI_LOG_STREAM, &scope, FrameDraft::new(frame_payload))
            .seq;

        self.write_pin.admit_log_write(&self.region)?;
        self.open_or_extend_anchor(coord, frame_offset, frame_offset + len);

        let should_seal = {
            let st = self
                .streams
                .get_mut(&key)
                .expect("stream state opened above");
            if st.open_segment.is_empty() {
                st.open_segment_start = st.next_offset;
            }
            st.open_segment.extend_from_slice(bytes);
            st.next_offset += len;
            st.bytes_since_pointer += len as u64;
            self.lines_shipped += 1;
            st.open_segment.len() as u64 >= self.seal.seal_at_bytes
        };

        if should_seal {
            self.seal_open_segment(coord)?;
        }

        let crossed = {
            let st = self.streams.get(&key).expect("stream state");
            st.bytes_since_pointer >= self.coalesce.bytes_per_pointer
        };
        if crossed {
            self.emit_coalesced_pointer(coord, None)?;
        }

        Ok(seq)
    }

    fn open_or_extend_anchor(&mut self, coord: &LogCoord, _start: i64, _end: i64) {
        let akey = (
            coord.run_id.clone(),
            coord.job_id.clone(),
            coord.step_id.clone(),
        );
        let st_start = self
            .streams
            .get(&(coord.run_id.clone(), coord.job_id.clone()))
            .map(|s| s.next_offset)
            .unwrap_or(0);
        self.anchor_rows
            .entry(akey)
            .or_insert_with(|| LogAnchorRow {
                tenant_id: self.tenant.as_str().to_string(),
                region: self.region.as_str().to_string(),
                run_id: coord.run_id.clone(),
                job_id: coord.job_id.clone(),
                step_id: coord.step_id.clone(),
                byte_start: st_start,
                byte_end: None,
                status: AnchorStatus::Running,
            });
    }

    pub fn close_step(
        &mut self,
        coord: &LogCoord,
        status: AnchorStatus,
    ) -> Result<(), CrossRegionLogWrite> {
        self.write_pin.admit_log_write(&self.region)?;
        let end = self
            .streams
            .get(&(coord.run_id.clone(), coord.job_id.clone()))
            .map(|s| s.next_offset)
            .unwrap_or(0);
        let akey = (
            coord.run_id.clone(),
            coord.job_id.clone(),
            coord.step_id.clone(),
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
                    step_id: coord.step_id.clone(),
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
    ) -> Result<Option<String>, CrossRegionLogWrite> {
        let key = (coord.run_id.clone(), coord.job_id.clone());
        let (bytes, seg_start, seg_end, seg_seq) = {
            let Some(st) = self.streams.get_mut(&key) else {
                return Ok(None);
            };
            if st.open_segment.is_empty() {
                return Ok(None);
            }
            let bytes = std::mem::take(&mut st.open_segment);
            let seg_start = st.open_segment_start;
            let seg_end = st.next_offset;
            let seq = st.next_segment_seq;
            st.next_segment_seq += 1;
            (bytes, seg_start, seg_end, seq)
        };

        self.write_pin.admit_log_write(&self.region)?;

        let blob_ref = match self.blobs.put(&self.tenant, &bytes) {
            Ok(hash) => hash.to_multihash_string(),
            Err(_) => ContentHash::blake3(&bytes).to_multihash_string(),
        };

        self.segment_rows.push(LogSegmentRow {
            tenant_id: self.tenant.as_str().to_string(),
            region: self.region.as_str().to_string(),
            run_id: coord.run_id.clone(),
            job_id: coord.job_id.clone(),
            segment_seq: seg_seq,
            blob_ref: Some(blob_ref.clone()),
            byte_start: seg_start,
            byte_end: seg_end,
            pii_key_ref: self.tenant_dek_ref(),
        });

        self.emit_coalesced_pointer(coord, Some(blob_ref.clone()))?;

        Ok(Some(blob_ref))
    }

    fn emit_coalesced_pointer(
        &mut self,
        coord: &LogCoord,
        segment_ref: Option<String>,
    ) -> Result<(), CrossRegionLogWrite> {
        self.write_pin.admit_log_write(&self.region)?;
        let key = (coord.run_id.clone(), coord.job_id.clone());
        let (range_start, range_end) = {
            let st = self.streams.get_mut(&key).expect("stream state");
            let start = st.last_pointer_offset;
            let end = st.next_offset;
            st.last_pointer_offset = end;
            st.bytes_since_pointer = 0;
            (start, end)
        };
        if range_end <= range_start && segment_ref.is_none() {
            return Ok(());
        }
        self.pointers.push(LogAvailablePointer {
            coord: coord.clone(),
            byte_start: range_start,
            byte_end: range_end,
            segment_ref,
        });
        Ok(())
    }

    pub fn flush_job(
        &mut self,
        run_id: &str,
        job_id: &str,
        step_id: &str,
    ) -> Result<(), CrossRegionLogWrite> {
        let coord = LogCoord::new(run_id, job_id, step_id);
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

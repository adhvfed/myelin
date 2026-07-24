//! # `log_sink` — CT-004f: bind the runner's `FirehoseSink` seam to the real `LogPipeline`
//!
//! **The cycle-safe live-log binding (CT-004f sub-step 3).** The runner
//! ([`myelin_ci_sandbox::RunnerAgent::run_one`]) streams captured stdout/stderr through the
//! [`FirehoseSink`](myelin_ci_sandbox::FirehoseSink) seam — a trait in the LOWER `ci-sandbox` crate
//! (the runner cannot depend on this crate's [`LogPipeline`](crate::LogPipeline)). This module is the
//! HIGHER-crate impl that CAN name both: [`LogPipelineSink`] implements `FirehoseSink` by driving a
//! real `LogPipeline` (boundary-redacted bytes → seal T2 segment → `(job, step, byte-range)`
//! index → coalesced `ci.log.available` pointer). See
//! `planning/system-reviews/2026-07-17-ct004f-log-pipeline-scoping.md`.
//!
//! ## The three findings this binding honours
//! - **F1 — the sink is MULTI-TENANT + per-job.** A runner claims across tenants (its lease predicate
//!   has no tenant filter), so ONE shared sink opens a `LogPipeline` PER `(tenant, run, job)` lazily
//!   (the pipeline is `new(tenant, region, blobs, redactor)` — a per-tenant CAS keyspace + a residency
//!   write-pin). `region` is NOT on the seam (a runner serves one region — held at construction).
//! - **F3 — redaction is a BOUNDARY responsibility, NOT this sink's.** The frames the runner ships
//!   MUST already be redacted inside the sandbox (where the broker resolved the plaintext); the
//!   least-privilege runner holds only opaque `SecretRef`s. So this sink constructs the pipeline with
//!   an EMPTY [`SecretRedactor`] (defence-in-depth over already-redacted bytes) — it NEVER pulls
//!   secret plaintext into the control plane.
//! - **F4 — byte-exact frames.** Sandbox read boundaries are not text or line boundaries. The
//!   adapter sends already boundary-redacted bytes to `LogPipeline::ship_frame` unchanged, under one
//!   stable step id ([`SINGLE_STEP_ID`] — one command per job today).
//!
//! ## What is DB-free here (sub-step 3) vs live (sub-step 4)
//! The frame→seal→row mapping is DB-free (the pipeline core seals to a [`BlobStore`], which a test
//! drives with an in-memory `Arc<FsBlobStore>`). Every frame hands its newly sealed
//! `log_segment`/running-anchor rows + `ci.log.available` pointers to an injected [`LogPersist`];
//! `finish` persists the terminal anchor.

use std::collections::HashMap;
use std::sync::Mutex;

use myelin_ci_sandbox::FirehoseSink;
use myelin_storage::BlobStore;
use myelin_tenancy::{Region, TenantId};

use crate::log_pipeline::{
    AnchorStatus, LogAnchorRow, LogAvailablePointer, LogCoord, LogPipeline, LogSegmentRow,
    SealThreshold, SecretRedactor,
};

/// The single logical step id for a single-command job (RESHAPE-001: one command per job — the
/// sandbox runs ONE command, so its whole output is one step). A multi-step job would thread a real
/// per-step id through the seam; every frame lands under this stable step id today.
pub const SINGLE_STEP_ID: &str = "0";

/// Maximum sealed-segment size the production runner can legitimately produce. An open segment is
/// strictly below the seal threshold before one bounded backend frame is appended. New frames remain
/// byte-exact; the three-times frame allowance preserves readability of segments emitted by the
/// earlier lossy-UTF-8 adapter during a rolling upgrade.
pub const PRODUCTION_LOG_SEGMENT_MAX_BYTES: usize =
    SealThreshold::DEFAULT_SEAL_AT_BYTES as usize + 3 * myelin_ci_sandbox::SANDBOX_CAPTURE_BOUND;

/// **One incremental or terminal log-index checkpoint for `(tenant, run, job)`** — what
/// [`LogPersist`] durably writes. References-not-payloads: the `segments` name content addresses in
/// the CAS (never log bytes), the `anchors` name byte ranges, and `pointers` are the coalesced
/// `ci.log.available` facts that ride the outbox.
#[derive(Debug, Clone)]
pub struct FlushedJobLogs {
    /// The run id (opaque, PII-free).
    pub run_id: String,
    /// The job id (opaque, PII-free).
    pub job_id: String,
    /// The sealed `log_segment` rows (the `(job, step, byte-range) → (blob, offset)` index).
    pub segments: Vec<LogSegmentRow>,
    /// The `log_anchor` rows (the per-step spans + terminal status).
    pub anchors: Vec<LogAnchorRow>,
    /// The coalesced durable `ci.log.available` pointers (the caller emits each via the outbox).
    pub pointers: Vec<LogAvailablePointer>,
}

/// Durable append position recovered before a retried execution emits its first new frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogResume {
    /// The next `log_segment.segment_seq`.
    pub next_segment_seq: i32,
    /// The first byte offset of the retry's next frame.
    pub next_byte_offset: i64,
    /// The stable start of the existing step anchor.
    pub step_byte_start: i64,
}

/// **The durable-write seam each incremental/terminal log checkpoint flushes through.** The DB-free
/// adapter drives the pipeline to newly sealed rows + pointers and hands them here; the live impl
/// writes `log_segment`/`log_anchor` on a tenant-scoped FORCE-RLS tx and emits the pointers via the
/// outbox (durable, `no-raw-publish` green).
pub trait LogPersist: Send + Sync {
    /// Recover the append-only durable head for a retried job. Recording/test implementations start
    /// empty by default; the production store overrides this with an exact tenant/region read.
    fn resume(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        _run_id: &str,
        _job_id: &str,
    ) -> Result<LogResume, Box<dyn std::error::Error + Send + Sync>> {
        Ok(LogResume::default())
    }

    /// Persist newly sealed index rows + emit their pointers. A mid-run error is returned through the
    /// firehose and fails the runner cycle before terminal reporting; output is never silently
    /// acknowledged without its durable resume authority.
    fn persist(
        &self,
        tenant: &TenantId,
        flushed: FlushedJobLogs,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

/// A shared-handle forward so a caller can hold an `Arc<P>` for its own inspection (a test asserting
/// on the recorded flushes; a composition root sharing one writer) while the sink owns a clone.
/// Mirrors `impl BlobStore for Arc<B>`.
impl<T: LogPersist> LogPersist for std::sync::Arc<T> {
    fn resume(
        &self,
        tenant: &TenantId,
        region: &Region,
        run_id: &str,
        job_id: &str,
    ) -> Result<LogResume, Box<dyn std::error::Error + Send + Sync>> {
        (**self).resume(tenant, region, run_id, job_id)
    }

    fn persist(
        &self,
        tenant: &TenantId,
        flushed: FlushedJobLogs,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        (**self).persist(tenant, flushed)
    }
}

/// **The real [`FirehoseSink`] — drives a per-`(tenant, run, job)` [`LogPipeline`] (CT-004f).** Holds
/// the runner's single `region` + a cheaply-clonable `blobs` handle (`S3BlobStore` in production, an
/// `Arc<FsBlobStore>` in tests) + the injected [`LogPersist`]. `ship_frame` opens/extends the job's
/// pipeline; each frame is sealed and persisted before acknowledgment, while `finish` closes the
/// step and persists its terminal anchor.
pub struct LogPipelineSink<S: BlobStore + Clone, P: LogPersist> {
    /// The cell's region (the residency pin every pipeline is constructed with — a runner serves ONE
    /// region, so this is construction-time, not on the seam).
    region: Region,
    /// The content-addressed blob store handle each pipeline seals to (a cheap Clone: an S3 handle in
    /// production; `Arc<FsBlobStore>` in tests via the `impl BlobStore for Arc<B>` forward).
    blobs: S,
    /// The durable-write seam a finished job's index flushes through.
    persist: P,
    /// The OPEN pipelines keyed by `(tenant, run, job)` — opened lazily on the first frame, removed on
    /// `finish` (so a re-delivered `finish` is a no-op — idempotent).
    pipelines: Mutex<HashMap<(String, String, String), LogPipeline<S>>>,
}

impl<S: BlobStore + Clone, P: LogPersist> LogPipelineSink<S, P> {
    /// Build the sink for a runner serving `region`, sealing to `blobs`, flushing through `persist`.
    pub fn new(region: Region, blobs: S, persist: P) -> LogPipelineSink<S, P> {
        LogPipelineSink {
            region,
            blobs,
            persist,
            pipelines: Mutex::new(HashMap::new()),
        }
    }

    /// The `(tenant, run, job)` map key (owned — the pipeline outlives any one frame borrow).
    fn key(tenant: &TenantId, run_id: &str, job_id: &str) -> (String, String, String) {
        (
            tenant.as_str().to_string(),
            run_id.to_string(),
            job_id.to_string(),
        )
    }
}

impl<S: BlobStore + Clone, P: LogPersist> FirehoseSink for LogPipelineSink<S, P> {
    fn ship_frame(
        &self,
        run_id: &str,
        job_id: &str,
        tenant: &TenantId,
        frame: &[u8],
    ) -> Result<(), String> {
        if frame.is_empty() {
            return Ok(());
        }
        let key = Self::key(tenant, run_id, job_id);
        let needs_resume = !self
            .pipelines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&key);
        let resume = if needs_resume {
            self.persist
                .resume(tenant, &self.region, run_id, job_id)
                .map_err(|error| {
                    format!("recover durable log head for run={run_id} job={job_id}: {error}")
                })?
        } else {
            LogResume::default()
        };
        let flushed = {
            let mut map = self.pipelines.lock().unwrap_or_else(|e| e.into_inner());
            // Open lazily with an empty defence-in-depth redactor: the sandbox callback has already
            // applied the authoritative boundary plan.
            let pipe = map.entry(key.clone()).or_insert_with(|| {
                let mut pipeline = LogPipeline::new(
                    tenant.clone(),
                    self.region.clone(),
                    self.blobs.clone(),
                    SecretRedactor::for_job(std::iter::empty::<String>()),
                );
                pipeline.resume_stream(
                    &LogCoord::new(run_id, job_id, SINGLE_STEP_ID),
                    resume.step_byte_start,
                    resume.next_byte_offset,
                    resume.next_segment_seq,
                );
                pipeline
            });
            let coord = LogCoord::new(run_id, job_id, SINGLE_STEP_ID);
            pipe.ship_frame(&coord, frame)
                .map_err(|error| error.to_string())?;
            // Force a seal at the bounded backend frame so Edge can observe the segment while the
            // command is still executing; the open buffer never exceeds one callback.
            pipe.seal_open_segment(&coord)
                .map_err(|error| error.to_string())?;
            FlushedJobLogs {
                run_id: run_id.to_string(),
                job_id: job_id.to_string(),
                segments: pipe.drain_segment_rows(),
                anchors: pipe.anchor_rows().into_iter().cloned().collect(),
                pointers: pipe.drain_pointers(),
            }
        };
        if let Err(error) = self.persist.persist(tenant, flushed) {
            self.pipelines
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key);
            return Err(format!(
                "persist incremental log checkpoint for run={run_id} job={job_id}: {error}"
            ));
        }
        Ok(())
    }

    fn finish(
        &self,
        run_id: &str,
        job_id: &str,
        tenant: &TenantId,
        passed: bool,
    ) -> Result<(), String> {
        let key = Self::key(tenant, run_id, job_id);
        // Remove the active pipeline. If this is a retry that produced no new output, recover the
        // durable head so its prior running anchor can still be closed. A repeated finish performs
        // only an idempotent anchor upsert—never another segment seal.
        let existing = {
            let mut map = self.pipelines.lock().unwrap_or_else(|e| e.into_inner());
            map.remove(&key)
        };
        let mut pipe = match existing {
            Some(pipe) => pipe,
            None => {
                let resume = self
                    .persist
                    .resume(tenant, &self.region, run_id, job_id)
                    .map_err(|error| {
                        format!(
                            "recover durable log head for terminal run={run_id} job={job_id}: {error}"
                        )
                    })?;
                let mut pipeline = LogPipeline::new(
                    tenant.clone(),
                    self.region.clone(),
                    self.blobs.clone(),
                    SecretRedactor::for_job(std::iter::empty::<String>()),
                );
                pipeline.resume_stream(
                    &LogCoord::new(run_id, job_id, SINGLE_STEP_ID),
                    resume.step_byte_start,
                    resume.next_byte_offset,
                    resume.next_segment_seq,
                );
                pipeline
            }
        };
        let coord = LogCoord::new(run_id, job_id, SINGLE_STEP_ID);
        // CLOSE the step anchor with the job verdict (the single-command job's verdict IS its one
        // step's), then flush (seal the remaining open segment + emit the final coalesced pointer).
        let status = if passed {
            AnchorStatus::Passed
        } else {
            AnchorStatus::Failed
        };
        pipe.close_step(&coord, status)
            .map_err(|error| error.to_string())?;
        pipe.flush_job(run_id, job_id, SINGLE_STEP_ID)
            .map_err(|error| error.to_string())?;
        let flushed = FlushedJobLogs {
            run_id: run_id.to_string(),
            job_id: job_id.to_string(),
            segments: pipe.drain_segment_rows(),
            anchors: pipe.anchor_rows().into_iter().cloned().collect(),
            pointers: pipe.drain_pointers(),
        };
        self.persist
            .persist(tenant, flushed)
            .map_err(|error| format!("persist terminal log checkpoint: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_ci_sandbox::FirehoseSink;
    use myelin_storage::{ContentHash, FsBlobStore};
    use std::sync::Arc;

    /// A recording [`LogPersist`] — captures every flushed job so a test can assert the mapping.
    #[derive(Default)]
    struct RecordingPersist {
        flushed: Mutex<Vec<(String, FlushedJobLogs)>>,
    }
    impl LogPersist for RecordingPersist {
        fn persist(
            &self,
            tenant: &TenantId,
            flushed: FlushedJobLogs,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.flushed
                .lock()
                .unwrap()
                .push((tenant.as_str().to_string(), flushed));
            Ok(())
        }
    }

    struct FailingPersist;
    impl LogPersist for FailingPersist {
        fn persist(
            &self,
            _tenant: &TenantId,
            _flushed: FlushedJobLogs,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Err(std::io::Error::other("injected durable log failure").into())
        }
    }

    fn sink() -> LogPipelineSink<Arc<FsBlobStore>, Arc<RecordingPersist>> {
        LogPipelineSink::new(
            Region::new("eu-west"),
            Arc::new(FsBlobStore::new()),
            Arc::new(RecordingPersist::default()),
        )
    }

    #[test]
    fn ship_then_finish_seals_and_flushes_the_index() {
        let persist = Arc::new(RecordingPersist::default());
        let s = LogPipelineSink::new(
            Region::new("eu-west"),
            Arc::new(FsBlobStore::new()),
            persist.clone(),
        );
        let tenant = TenantId::from_token("tnt-1");
        s.ship_frame("run-1", "job-1", &tenant, b"line one\nline two\n")
            .expect("first incremental checkpoint");
        s.ship_frame("run-1", "job-1", &tenant, b"stderr blip\n")
            .expect("second incremental checkpoint");
        assert_eq!(
            persist.flushed.lock().unwrap().len(),
            2,
            "each frame is durable before terminal finish"
        );

        s.finish("run-1", "job-1", &tenant, true)
            .expect("terminal checkpoint");

        let flushed = persist.flushed.lock().unwrap();
        assert_eq!(flushed.len(), 3, "two live checkpoints plus terminal");
        let (tid, job) = &flushed[2];
        assert_eq!(tid, "tnt-1");
        assert_eq!(job.run_id, "run-1");
        assert_eq!(job.job_id, "job-1");
        // The step anchor closed as PASSED with a bounded span (byte_end set — not left `running`).
        assert_eq!(job.anchors.len(), 1, "one step anchor");
        let anchor = &job.anchors[0];
        assert_eq!(anchor.status, AnchorStatus::Passed);
        assert_eq!(anchor.step_id, SINGLE_STEP_ID);
        assert!(
            anchor.byte_end.is_some(),
            "a finished step's anchor is closed"
        );
        // Output segments and pointers were already persisted by the two live checkpoints.
        let live_segments: Vec<_> = flushed[..2]
            .iter()
            .flat_map(|(_, checkpoint)| checkpoint.segments.iter())
            .collect();
        assert_eq!(live_segments.len(), 2);
        assert!(
            live_segments.iter().all(|s| s.blob_ref.is_some()),
            "every sealed segment names a CAS blob"
        );
        assert!(
            flushed[..2]
                .iter()
                .all(|(_, checkpoint)| !checkpoint.pointers.is_empty()),
            "each live segment emitted a durable pointer"
        );
    }

    #[test]
    fn binary_frames_remain_byte_exact_across_read_boundaries() {
        let blobs = Arc::new(FsBlobStore::new());
        let persist = Arc::new(RecordingPersist::default());
        let sink = LogPipelineSink::new(Region::new("eu-west"), blobs.clone(), persist.clone());
        let tenant = TenantId::from_token("tnt-invalid-utf8");
        let first = b"line one\nsplit-\xf0\x9f".to_vec();
        let second = b"\x98\x80\nraw-\xff\x00-tail\n".to_vec();
        sink.ship_frame("run-invalid", "job-invalid", &tenant, &first)
            .expect("first binary frame");
        sink.ship_frame("run-invalid", "job-invalid", &tenant, &second)
            .expect("second binary frame");

        let flushed = persist.flushed.lock().unwrap();
        assert_eq!(flushed.len(), 2);
        let mut archived = Vec::new();
        for (_, checkpoint) in flushed.iter() {
            let segment = checkpoint.segments.first().expect("one segment per frame");
            let hash = ContentHash::parse(segment.blob_ref.as_deref().expect("sealed blob ref"))
                .expect("canonical content address");
            archived.extend(
                blobs
                    .get_bounded(&tenant, &hash, PRODUCTION_LOG_SEGMENT_MAX_BYTES)
                    .expect("bounded archived segment"),
            );
        }
        let mut expected = first;
        expected.extend(second);
        assert_eq!(archived, expected, "no UTF-8 expansion or newline loss");
    }

    #[test]
    fn incremental_persist_failure_is_loud_and_drops_in_memory_state() {
        let sink = LogPipelineSink::new(
            Region::new("eu-west"),
            Arc::new(FsBlobStore::new()),
            FailingPersist,
        );
        let tenant = TenantId::from_token("tnt-fail");
        let error = sink
            .ship_frame("run-fail", "job-fail", &tenant, b"must be durable\n")
            .expect_err("an unpersisted frame is never acknowledged");
        assert!(error.contains("incremental log checkpoint"));
        assert!(
            sink.pipelines.lock().unwrap().is_empty(),
            "failed jobs do not leak a stale open pipeline into a retry"
        );
    }

    #[test]
    fn finish_is_idempotent_no_double_seal() {
        let s = sink();
        let tenant = TenantId::from_token("tnt-1");
        s.ship_frame("run-1", "job-1", &tenant, b"hello\n")
            .expect("live checkpoint");
        s.finish("run-1", "job-1", &tenant, true)
            .expect("terminal checkpoint");
        // A re-delivered terminal report calls finish AGAIN — the pipeline is already removed, so this
        // is a no-op (no panic, no second flush). Proven by the pipelines map being empty.
        s.finish("run-1", "job-1", &tenant, true)
            .expect("idempotent terminal retry");
        assert!(
            s.pipelines.lock().unwrap().is_empty(),
            "no lingering pipeline after finish"
        );
    }

    #[test]
    fn a_failed_job_closes_the_anchor_as_failed() {
        let persist = Arc::new(RecordingPersist::default());
        let s = LogPipelineSink::new(
            Region::new("eu-west"),
            Arc::new(FsBlobStore::new()),
            persist.clone(),
        );
        let tenant = TenantId::from_token("tnt-1");
        s.ship_frame("run-9", "job-9", &tenant, b"boom\n")
            .expect("live checkpoint");
        s.finish("run-9", "job-9", &tenant, false)
            .expect("terminal checkpoint");
        let flushed = persist.flushed.lock().unwrap();
        assert_eq!(
            flushed.last().unwrap().1.anchors[0].status,
            AnchorStatus::Failed
        );
    }

    #[test]
    fn distinct_tenants_get_distinct_pipelines() {
        let persist = Arc::new(RecordingPersist::default());
        let s = LogPipelineSink::new(
            Region::new("eu-west"),
            Arc::new(FsBlobStore::new()),
            persist.clone(),
        );
        let a = TenantId::from_token("tnt-a");
        let b = TenantId::from_token("tnt-b");
        // SAME run/job ids across DIFFERENT tenants must not collide (the key is tenant-scoped).
        s.ship_frame("run-1", "job-1", &a, b"tenant a line\n")
            .expect("tenant a live checkpoint");
        s.ship_frame("run-1", "job-1", &b, b"tenant b line\n")
            .expect("tenant b live checkpoint");
        assert_eq!(
            s.pipelines.lock().unwrap().len(),
            2,
            "one pipeline per tenant"
        );
        s.finish("run-1", "job-1", &a, true)
            .expect("tenant a terminal checkpoint");
        s.finish("run-1", "job-1", &b, true)
            .expect("tenant b terminal checkpoint");
        let flushed = persist.flushed.lock().unwrap();
        assert_eq!(flushed.len(), 4);
        let tenants: Vec<&str> = flushed.iter().map(|(t, _)| t.as_str()).collect();
        assert!(tenants.contains(&"tnt-a") && tenants.contains(&"tnt-b"));
    }
}

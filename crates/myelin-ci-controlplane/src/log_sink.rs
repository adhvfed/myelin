//! # `log_sink` — CT-004f: bind the runner's `FirehoseSink` seam to the real `LogPipeline`
//!
//! **The cycle-safe live-log binding (CT-004f sub-step 3).** The runner
//! ([`myelin_ci_sandbox::RunnerAgent::run_one`]) streams captured stdout/stderr through the
//! [`FirehoseSink`](myelin_ci_sandbox::FirehoseSink) seam — a trait in the LOWER `ci-sandbox` crate
//! (the runner cannot depend on this crate's [`LogPipeline`](crate::LogPipeline)). This module is the
//! HIGHER-crate impl that CAN name both: [`LogPipelineSink`] implements `FirehoseSink` by driving a
//! real `LogPipeline` (redact → firehose live-tail → seal T2 segment → `(job, step, byte-range)`
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
//! - **F4 — frame→line impedance.** `ship_frame` ships whole streams as `&[u8]`; `LogPipeline`
//!   `ship_line`s `&str` lines. This adapter does `from_utf8_lossy(frame).lines()` → `ship_line`,
//!   under a single stable step id ([`SINGLE_STEP_ID`] — one command per job today; a multi-step job
//!   would carry a real step id).
//!
//! ## What is DB-free here (sub-step 3) vs live (sub-step 4)
//! The frame→seal→row mapping is DB-free (the pipeline core seals to a [`BlobStore`], which a test
//! drives with an in-memory `Arc<FsBlobStore>`). On [`finish`](LogPipelineSink), the sealed
//! `log_segment`/`log_anchor` rows + the drained `ci.log.available` pointers are handed to an injected
//! [`LogPersist`] — a recording stub in tests; the live impl (a tenant-scoped FORCE-RLS `log_segment`/
//! `log_anchor` write + an outbox pointer emit) is **CT-004f sub-step 4**.

use std::collections::HashMap;
use std::sync::Mutex;

use myelin_ci_sandbox::FirehoseSink;
use myelin_storage::BlobStore;
use myelin_tenancy::{Region, TenantId};

use crate::log_pipeline::{
    AnchorStatus, LogAnchorRow, LogAvailablePointer, LogCoord, LogPipeline, LogSegmentRow,
    SecretRedactor,
};

/// The single logical step id for a single-command job (RESHAPE-001: one command per job — the
/// sandbox runs ONE command, so its whole output is one step). A multi-step job would thread a real
/// per-step id through the seam; every frame lands under this stable step id today.
pub const SINGLE_STEP_ID: &str = "0";

/// **The sealed index + drained pointers for one finished `(tenant, run, job)`** — what
/// [`LogPersist`] durably writes. References-not-payloads: the `segments` name content addresses in
/// the CAS (never log bytes), the `anchors` name byte ranges, the `pointers` are the coalesced
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

/// **The durable-write seam a finished job's log index flushes through (CT-004f sub-step 4 fills it).**
/// The DB-free adapter (sub-step 3) drives the pipeline to sealed rows + pointers and hands them here;
/// the live impl writes `log_segment`/`log_anchor` on a tenant-scoped FORCE-RLS tx and emits the
/// pointers via the outbox (durable, `no-raw-publish` green). A recording stub proves the mapping in
/// tests.
pub trait LogPersist: Send + Sync {
    /// Persist the sealed index + emit the pointers for a finished `(tenant, run, job)`. Returns the
    /// durable-write error so the sink can surface it (a lost log index is diagnosable, never a silent
    /// swallow) — the fail-loud-vs-best-effort policy for a persist failure mid-run is settled in
    /// sub-step 4 (the runner's job already ran; only the log index is at stake).
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
/// pipeline; `finish` closes the step, seals, and flushes the index through `LogPersist`.
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
    fn ship_frame(&self, run_id: &str, job_id: &str, tenant: &TenantId, frame: &[u8]) {
        let key = Self::key(tenant, run_id, job_id);
        let mut map = self.pipelines.lock().unwrap_or_else(|e| e.into_inner());
        // Open the job's pipeline lazily (per-tenant CAS keyspace + residency pin). EMPTY redactor:
        // redaction is a BOUNDARY responsibility (F3) — these frames are already redacted; the
        // pipeline redactor is defence-in-depth, and this control-plane process never holds the
        // plaintext needles a real redactor would need.
        let pipe = map.entry(key).or_insert_with(|| {
            LogPipeline::new(
                tenant.clone(),
                self.region.clone(),
                self.blobs.clone(),
                SecretRedactor::for_job(std::iter::empty::<String>()),
            )
        });
        // F4: a frame is a captured byte chunk; the pipeline is line-oriented. Lossy-decode (log bytes
        // may be non-UTF8) and ship each line. `str::lines()` yields the final line even without a
        // trailing newline. A cross-region write cannot occur (the pipeline's region == self.region),
        // so `ship_line` never errs here.
        let text = String::from_utf8_lossy(frame);
        let coord = LogCoord::new(run_id, job_id, SINGLE_STEP_ID);
        for line in text.lines() {
            let _ = pipe.ship_line(&coord, line);
        }
    }

    fn finish(&self, run_id: &str, job_id: &str, tenant: &TenantId, passed: bool) {
        let key = Self::key(tenant, run_id, job_id);
        // Remove the pipeline: a re-delivered `finish` finds nothing → no double seal (idempotent).
        let mut pipe = {
            let mut map = self.pipelines.lock().unwrap_or_else(|e| e.into_inner());
            match map.remove(&key) {
                Some(p) => p,
                None => return,
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
        let _ = pipe.close_step(&coord, status);
        let _ = pipe.flush_job(run_id, job_id, SINGLE_STEP_ID);
        let flushed = FlushedJobLogs {
            run_id: run_id.to_string(),
            job_id: job_id.to_string(),
            segments: pipe.segment_rows().to_vec(),
            anchors: pipe.anchor_rows().into_iter().cloned().collect(),
            pointers: pipe.drain_pointers(),
        };
        if let Err(e) = self.persist.persist(tenant, flushed) {
            // Surface, never silently swallow: a lost log index is diagnosable. The job itself already
            // ran (the terminal report is independent); the fail-loud-vs-best-effort policy for a
            // persist failure is settled in sub-step 4 when the live writer lands.
            eprintln!(
                "log_sink: persist failed for run={run_id} job={job_id} (log index may be \
                 incomplete): {e}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_ci_sandbox::FirehoseSink;
    use myelin_storage::FsBlobStore;
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
        s.ship_frame("run-1", "job-1", &tenant, b"line one\nline two\n");
        s.ship_frame("run-1", "job-1", &tenant, b"stderr blip\n");
        // Before finish: nothing has been flushed (finish is the terminal seal).
        assert_eq!(persist.flushed.lock().unwrap().len(), 0);

        s.finish("run-1", "job-1", &tenant, true);

        let flushed = persist.flushed.lock().unwrap();
        assert_eq!(flushed.len(), 1, "one job flushed");
        let (tid, job) = &flushed[0];
        assert_eq!(tid, "tnt-1");
        assert_eq!(job.run_id, "run-1");
        assert_eq!(job.job_id, "job-1");
        // The step anchor closed as PASSED with a bounded span (byte_end set — not left `running`).
        assert_eq!(job.anchors.len(), 1, "one step anchor");
        let anchor = &job.anchors[0];
        assert_eq!(anchor.status, AnchorStatus::Passed);
        assert_eq!(anchor.step_id, SINGLE_STEP_ID);
        assert!(anchor.byte_end.is_some(), "a finished step's anchor is closed");
        // The output sealed to at least one content-addressed segment + a durable pointer.
        assert!(!job.segments.is_empty(), "output sealed to a segment");
        assert!(
            job.segments.iter().all(|s| s.blob_ref.is_some()),
            "every sealed segment names a CAS blob"
        );
        assert!(!job.pointers.is_empty(), "a ci.log.available pointer was emitted");
    }

    #[test]
    fn finish_is_idempotent_no_double_seal() {
        let s = sink();
        let tenant = TenantId::from_token("tnt-1");
        s.ship_frame("run-1", "job-1", &tenant, b"hello\n");
        s.finish("run-1", "job-1", &tenant, true);
        // A re-delivered terminal report calls finish AGAIN — the pipeline is already removed, so this
        // is a no-op (no panic, no second flush). Proven by the pipelines map being empty.
        s.finish("run-1", "job-1", &tenant, true);
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
        s.ship_frame("run-9", "job-9", &tenant, b"boom\n");
        s.finish("run-9", "job-9", &tenant, false);
        let flushed = persist.flushed.lock().unwrap();
        assert_eq!(flushed[0].1.anchors[0].status, AnchorStatus::Failed);
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
        s.ship_frame("run-1", "job-1", &a, b"tenant a line\n");
        s.ship_frame("run-1", "job-1", &b, b"tenant b line\n");
        assert_eq!(s.pipelines.lock().unwrap().len(), 2, "one pipeline per tenant");
        s.finish("run-1", "job-1", &a, true);
        s.finish("run-1", "job-1", &b, true);
        let flushed = persist.flushed.lock().unwrap();
        assert_eq!(flushed.len(), 2);
        let tenants: Vec<&str> = flushed.iter().map(|(t, _)| t.as_str()).collect();
        assert!(tenants.contains(&"tnt-a") && tenants.contains(&"tnt-b"));
    }
}

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_ci_sandbox::FirehoseSink;
use myelin_storage::BlobStore;
use myelin_tenancy::{Region, TenantId};

use crate::log_pipeline::{
    AnchorStatus, LogAnchorRow, LogAvailablePointer, LogCoord, LogPipeline, LogSegmentRow,
    SealThreshold, SecretRedactor,
};

pub const SINGLE_STEP_NO: u32 = 0;

pub const PRODUCTION_LOG_SEGMENT_MAX_BYTES: usize =
    SealThreshold::DEFAULT_SEAL_AT_BYTES as usize + 3 * myelin_ci_sandbox::SANDBOX_CAPTURE_BOUND;

#[derive(Debug, Clone)]
pub struct FlushedJobLogs {
    pub run_id: String,
    pub job_id: String,
    pub segments: Vec<LogSegmentRow>,
    pub anchors: Vec<LogAnchorRow>,
    pub pointers: Vec<LogAvailablePointer>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogResume {
    pub canonical_run_id: Option<String>,
    pub next_segment_seq: i32,
    pub next_byte_offset: i64,
    pub step_byte_start: i64,
}

pub trait LogPersist: Send + Sync {
    fn resume(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        _run_id: &str,
        _job_id: &str,
    ) -> Result<LogResume, Box<dyn std::error::Error + Send + Sync>> {
        Ok(LogResume::default())
    }

    fn persist(
        &self,
        tenant: &TenantId,
        flushed: FlushedJobLogs,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

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

pub struct LogPipelineSink<S: BlobStore + Clone, P: LogPersist> {
    region: Region,
    blobs: S,
    persist: P,
    pipelines: Mutex<HashMap<PipelineKey, Arc<Mutex<OpenLogPipeline<S>>>>>,
}

type PipelineKey = (String, String, String);

struct OpenLogPipeline<S: BlobStore + Clone> {
    canonical_run_id: String,
    pipeline: LogPipeline<S>,
    accepting_frames: bool,
}

impl<S: BlobStore + Clone, P: LogPersist> LogPipelineSink<S, P> {
    pub fn new(region: Region, blobs: S, persist: P) -> LogPipelineSink<S, P> {
        LogPipelineSink {
            region,
            blobs,
            persist,
            pipelines: Mutex::new(HashMap::new()),
        }
    }

    fn key(tenant: &TenantId, run_id: &str, job_id: &str) -> PipelineKey {
        (
            tenant.as_str().to_string(),
            run_id.to_string(),
            job_id.to_string(),
        )
    }

    fn pipeline_for(
        &self,
        key: &PipelineKey,
        tenant: &TenantId,
        run_id: &str,
        job_id: &str,
    ) -> Result<Arc<Mutex<OpenLogPipeline<S>>>, String> {
        if let Some(open) = self
            .pipelines
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(key)
            .cloned()
        {
            return Ok(open);
        }

        let resume = self
            .persist
            .resume(tenant, &self.region, run_id, job_id)
            .map_err(|error| {
                format!("recover durable log head for run={run_id} job={job_id}: {error}")
            })?;
        let canonical_run_id = resume
            .canonical_run_id
            .unwrap_or_else(|| run_id.to_string());
        let mut pipeline = LogPipeline::new(
            tenant.clone(),
            self.region.clone(),
            self.blobs.clone(),
            SecretRedactor::for_job(std::iter::empty::<String>()),
        );
        pipeline
            .resume_stream(
                &LogCoord::new(&canonical_run_id, job_id, SINGLE_STEP_NO),
                resume.step_byte_start,
                resume.next_byte_offset,
                resume.next_segment_seq,
            )
            .map_err(|error| {
                format!("resume log pipeline for run={run_id} job={job_id}: {error}")
            })?;
        let candidate = Arc::new(Mutex::new(OpenLogPipeline {
            canonical_run_id,
            pipeline,
            accepting_frames: true,
        }));

        let mut pipelines = self
            .pipelines
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Ok(pipelines.entry(key.clone()).or_insert(candidate).clone())
    }

    fn retire(&self, key: &PipelineKey, expected: &Arc<Mutex<OpenLogPipeline<S>>>) {
        let mut pipelines = self
            .pipelines
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if pipelines
            .get(key)
            .is_some_and(|open| Arc::ptr_eq(open, expected))
        {
            pipelines.remove(key);
        }
    }
}

impl<S: BlobStore + Clone + Send + Sync, P: LogPersist> FirehoseSink for LogPipelineSink<S, P> {
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
        let handle = self.pipeline_for(&key, tenant, run_id, job_id)?;
        let mut open = handle.lock().unwrap_or_else(|error| error.into_inner());
        if !open.accepting_frames {
            return Err(format!(
                "log checkpoint for run={run_id} job={job_id} is no longer accepting frames; retry against its durable head"
            ));
        }
        let canonical_run_id = open.canonical_run_id.clone();
        let coord = LogCoord::new(&canonical_run_id, job_id, SINGLE_STEP_NO);
        if let Err(error) = open
            .pipeline
            .ship_frame(&coord, frame)
            .and_then(|_| open.pipeline.seal_open_segment(&coord))
        {
            open.accepting_frames = false;
            drop(open);
            self.retire(&key, &handle);
            return Err(format!(
                "prepare incremental log checkpoint for run={run_id} job={job_id}: {error}"
            ));
        }
        let flushed = FlushedJobLogs {
            run_id: open.canonical_run_id.clone(),
            job_id: job_id.to_string(),
            segments: open.pipeline.drain_segment_rows(),
            anchors: open.pipeline.anchor_rows().into_iter().cloned().collect(),
            pointers: open.pipeline.drain_pointers(),
        };
        if let Err(error) = self.persist.persist(tenant, flushed) {
            open.accepting_frames = false;
            drop(open);
            self.retire(&key, &handle);
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
        let handle = self.pipeline_for(&key, tenant, run_id, job_id)?;
        let mut open = handle.lock().unwrap_or_else(|error| error.into_inner());
        if !open.accepting_frames {
            return Err(format!(
                "log checkpoint for run={run_id} job={job_id} is already finishing"
            ));
        }
        let canonical_run_id = open.canonical_run_id.clone();
        let coord = LogCoord::new(&canonical_run_id, job_id, SINGLE_STEP_NO);
        let status = if passed {
            AnchorStatus::Passed
        } else {
            AnchorStatus::Failed
        };
        let prepared = open.pipeline.close_step(&coord, status).and_then(|_| {
            open.pipeline
                .flush_job(&canonical_run_id, job_id, SINGLE_STEP_NO)
        });
        if let Err(error) = prepared {
            open.accepting_frames = false;
            drop(open);
            self.retire(&key, &handle);
            return Err(format!(
                "prepare terminal log checkpoint for run={run_id} job={job_id}: {error}"
            ));
        }
        let flushed = FlushedJobLogs {
            run_id: open.canonical_run_id.clone(),
            job_id: job_id.to_string(),
            segments: open.pipeline.drain_segment_rows(),
            anchors: open.pipeline.anchor_rows().into_iter().cloned().collect(),
            pointers: open.pipeline.drain_pointers(),
        };
        let persisted = self.persist.persist(tenant, flushed);
        open.accepting_frames = false;
        drop(open);
        self.retire(&key, &handle);
        persisted.map_err(|error| format!("persist terminal log checkpoint: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_ci_sandbox::FirehoseSink;
    use myelin_storage::blob::BlobDependencyError;
    use myelin_storage::{BlobError, BlobMeta, ContentHash, FsBlobStore};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar};
    use std::time::Duration;

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

    #[derive(Clone)]
    struct FailOnceBlobStore {
        attempts: Arc<AtomicUsize>,
        inner: Arc<FsBlobStore>,
    }

    #[derive(Default)]
    struct BlockingFirstPersist {
        state: Mutex<BlockingPersistState>,
        changed: Condvar,
    }

    #[derive(Default)]
    struct BlockingPersistState {
        entered: Vec<(String, i64, i64)>,
        release_first: bool,
    }

    impl BlockingFirstPersist {
        fn wait_for_calls(&self, count: usize, timeout: Duration) -> bool {
            let state = self.state.lock().unwrap();
            let (state, _) = self
                .changed
                .wait_timeout_while(state, timeout, |state| state.entered.len() < count)
                .unwrap();
            state.entered.len() >= count
        }

        fn release_first(&self) {
            self.state.lock().unwrap().release_first = true;
            self.changed.notify_all();
        }

        fn entered(&self) -> Vec<(String, i64, i64)> {
            self.state.lock().unwrap().entered.clone()
        }
    }

    impl LogPersist for BlockingFirstPersist {
        fn persist(
            &self,
            _tenant: &TenantId,
            flushed: FlushedJobLogs,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let segment = flushed.segments.first().expect("one segment per frame");
            let mut state = self.state.lock().unwrap();
            state
                .entered
                .push((flushed.job_id, segment.byte_start, segment.byte_end));
            let is_first = state.entered.len() == 1;
            self.changed.notify_all();
            while is_first && !state.release_first {
                state = self.changed.wait(state).unwrap();
            }
            Ok(())
        }
    }

    impl BlobStore for FailOnceBlobStore {
        fn put(&self, tenant: &TenantId, bytes: &[u8]) -> Result<ContentHash, BlobError> {
            if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(BlobError::Backend(BlobDependencyError::Transient));
            }
            self.inner.put(tenant, bytes)
        }

        fn get(&self, tenant: &TenantId, hash: &ContentHash) -> Result<Vec<u8>, BlobError> {
            self.inner.get(tenant, hash)
        }

        fn head(&self, tenant: &TenantId, hash: &ContentHash) -> Result<BlobMeta, BlobError> {
            self.inner.head(tenant, hash)
        }

        fn delete(&self, tenant: &TenantId, hash: &ContentHash) -> Result<(), BlobError> {
            self.inner.delete(tenant, hash)
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
        assert_eq!(job.anchors.len(), 1, "one step anchor");
        let anchor = &job.anchors[0];
        assert_eq!(anchor.status, AnchorStatus::Passed);
        assert_eq!(anchor.step_id, SINGLE_STEP_NO.to_string());
        assert!(
            anchor.byte_end.is_some(),
            "a finished step's anchor is closed"
        );
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
    fn archive_failure_drops_the_partial_pipeline_and_retry_persists_one_copy() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let blobs = FailOnceBlobStore {
            attempts: attempts.clone(),
            inner: Arc::new(FsBlobStore::new()),
        };
        let persist = Arc::new(RecordingPersist::default());
        let sink = LogPipelineSink::new(Region::new("eu-west"), blobs, persist.clone());
        let tenant = TenantId::from_token("tnt-archive-retry");
        let frame = b"persist me exactly once\n";

        let error = sink
            .ship_frame("run-retry", "job-retry", &tenant, frame)
            .expect_err("object storage refusal is returned to the runner");
        assert!(error.contains("prepare incremental log checkpoint"));
        assert!(error.contains("object-store dependency is temporarily unavailable"));
        assert!(sink.pipelines.lock().unwrap().is_empty());
        assert!(persist.flushed.lock().unwrap().is_empty());

        sink.ship_frame("run-retry", "job-retry", &tenant, frame)
            .expect("the runner can retry the same frame");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        let flushed = persist.flushed.lock().unwrap();
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0].1.segments.len(), 1);
        assert_eq!(flushed[0].1.segments[0].byte_start, 0);
        assert_eq!(flushed[0].1.segments[0].byte_end, frame.len() as i64);
    }

    #[test]
    fn one_jobs_checkpoints_stay_ordered_while_another_job_can_persist() {
        let persist = Arc::new(BlockingFirstPersist::default());
        let sink = Arc::new(LogPipelineSink::new(
            Region::new("eu-west"),
            Arc::new(FsBlobStore::new()),
            persist.clone(),
        ));
        let tenant = TenantId::from_token("tnt-concurrent-logs");

        let first = {
            let sink = sink.clone();
            let tenant = tenant.clone();
            std::thread::spawn(move || sink.ship_frame("run-1", "job-1", &tenant, b"A"))
        };
        assert!(
            persist.wait_for_calls(1, Duration::from_secs(2)),
            "the first checkpoint reaches durable persistence"
        );

        let same_job = {
            let sink = sink.clone();
            let tenant = tenant.clone();
            std::thread::spawn(move || sink.ship_frame("run-1", "job-1", &tenant, b"B"))
        };
        assert!(
            !persist.wait_for_calls(2, Duration::from_millis(100)),
            "the next range of the same job waits for its predecessor"
        );

        let other_job = {
            let sink = sink.clone();
            let tenant = tenant.clone();
            std::thread::spawn(move || sink.ship_frame("run-1", "job-2", &tenant, b"C"))
        };
        assert!(
            persist.wait_for_calls(2, Duration::from_secs(2)),
            "an unrelated job is not serialized behind job-1"
        );

        persist.release_first();
        first.join().unwrap().unwrap();
        same_job.join().unwrap().unwrap();
        other_job.join().unwrap().unwrap();

        let entered = persist.entered();
        let job_one: Vec<_> = entered
            .iter()
            .filter(|(job, _, _)| job == "job-1")
            .map(|(_, start, end)| (*start, *end))
            .collect();
        assert_eq!(job_one, [(0, 1), (1, 2)]);
        assert!(entered.iter().any(|entry| entry == &("job-2".into(), 0, 1)));
    }

    #[test]
    fn finish_is_idempotent_no_double_seal() {
        let s = sink();
        let tenant = TenantId::from_token("tnt-1");
        s.ship_frame("run-1", "job-1", &tenant, b"hello\n")
            .expect("live checkpoint");
        s.finish("run-1", "job-1", &tenant, true)
            .expect("terminal checkpoint");
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

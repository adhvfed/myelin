use super::*;
use crate::launch_gate::{DirectChildRetirement, SandboxCommand, SpawnPhase};
use crate::redaction::RedactionPlan;
use crate::user_namespace::RunscInvocationMode;
use crate::{
    drain_capped, JobSpec, LaunchPermit, ResourceUsage, SandboxOutputSink, SandboxOutputStream,
    SandboxResult, SANDBOX_CAPTURE_BOUND,
};
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(super) static NEVER_CANCELLED: AtomicBool = AtomicBool::new(false);

pub(super) struct RunscOutcome {
    pub(super) exit: Option<i32>,
    pub(super) timed_out: bool,
    pub(super) stdout: Vec<u8>,
    pub(super) stdout_truncated: bool,
    pub(super) stderr: Vec<u8>,
    pub(super) wall: Duration,
    pub(super) cpu_seconds: Option<u64>,
    pub(super) stream_error: Option<String>,
}

#[derive(Debug)]
pub(crate) enum RunFailure {
    Uncommitted {
        message: String,
    },
    CommitOutcomeUnknown {
        message: String,
    },
    CommittedButNotExecuted {
        message: String,
    },
    Executed {
        message: String,
        usage: ResourceUsage,
    },
}

impl RunFailure {
    pub(super) fn uncommitted(message: impl Into<String>) -> Self {
        Self::Uncommitted {
            message: message.into(),
        }
    }

    pub(super) fn commit_outcome_unknown(message: impl Into<String>) -> Self {
        Self::CommitOutcomeUnknown {
            message: message.into(),
        }
    }

    pub(super) fn committed_but_not_executed(message: impl Into<String>) -> Self {
        Self::CommittedButNotExecuted {
            message: message.into(),
        }
    }

    pub(super) fn executed(message: impl Into<String>, usage: ResourceUsage) -> Self {
        Self::Executed {
            message: message.into(),
            usage,
        }
    }
}

impl std::fmt::Display for RunFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunFailure::Uncommitted { message }
            | RunFailure::CommitOutcomeUnknown { message }
            | RunFailure::CommittedButNotExecuted { message }
            | RunFailure::Executed { message, .. } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for RunFailure {}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_and_capture(
    bin: &Path,
    bundle: &Path,
    container_id: &str,
    timeout: Duration,
    mem_bytes: u64,
    options: RunCaptureOptions<'_>,
    launch_permit: Option<LaunchPermit>,
    mode: RunscInvocationMode,
    cgroup: &MemoryCgroup,
) -> (Result<RunscOutcome, RunFailure>, DirectChildRetirement) {
    let mut child_retirement = DirectChildRetirement::NoChildReturned;
    let result = run_and_capture_impl(
        bin,
        bundle,
        container_id,
        timeout,
        mem_bytes,
        options,
        launch_permit,
        mode,
        cgroup,
        &mut child_retirement,
    );
    (result, child_retirement)
}

#[allow(clippy::too_many_arguments)]
fn run_and_capture_impl(
    bin: &Path,
    bundle: &Path,
    container_id: &str,
    timeout: Duration,
    mem_bytes: u64,
    options: RunCaptureOptions<'_>,
    launch_permit: Option<LaunchPermit>,
    mode: RunscInvocationMode,
    cgroup: &MemoryCgroup,
    child_retirement: &mut DirectChildRetirement,
) -> Result<RunscOutcome, RunFailure> {
    let RunCaptureOptions {
        stdin,
        stdout_mode,
        cancellation,
        redaction,
        output,
    } = options;
    let has_streaming_output = output.is_some();

    let watchdog_timeout = launch_permit.as_ref().map(|_| timeout);
    let mut sandbox_command = SandboxCommand::new(bin, launch_permit, watchdog_timeout)
        .map_err(|error| RunFailure::uncommitted(format!("prepare runsc launch gate: {error}")))?;
    let fenced = sandbox_command.is_fenced();
    if fenced && stdin.is_some() {
        sandbox_command
            .return_stdin_to_caller_after_gate()
            .map_err(|error| {
                RunFailure::uncommitted(format!("retain runsc stdin after launch gate: {error}"))
            })?;
    }
    {
        let cmd = sandbox_command.command_mut();
        apply_runsc_invocation_policy(cmd, bin, mode).map_err(RunFailure::uncommitted)?;
        cmd.arg("--network=none")
            .arg("run")
            .arg("-bundle")
            .arg(bundle)
            .arg(container_id)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !fenced {
            cmd.stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        }
        cgroup.place_child(cmd).map_err(|e| {
            RunFailure::uncommitted(format!("bind runsc into the memory cgroup: {e}"))
        })?;
    }
    if fenced {
        let kill_file = cgroup.kill_file().map_err(|error| {
            RunFailure::uncommitted(format!("open runsc cgroup kill switch: {error}"))
        })?;
        sandbox_command
            .kill_cgroup_on_liveness_loss(kill_file)
            .map_err(|error| {
                RunFailure::uncommitted(format!("arm runsc cgroup kill switch: {error}"))
            })?;
    }
    let mut child = sandbox_command.spawn().map_err(|spawn_failure| {
        let message = format!("spawn runsc: {}", spawn_failure.message());
        *child_retirement = spawn_failure.child_retirement().clone();
        match spawn_failure.phase() {
            SpawnPhase::Uncommitted => RunFailure::uncommitted(message),
            SpawnPhase::CommitOutcomeUnknown => RunFailure::commit_outcome_unknown(message),
            SpawnPhase::CommittedButNotExecuted => RunFailure::committed_but_not_executed(message),
            SpawnPhase::Executed => {
                let elapsed = spawn_failure
                    .executed_at()
                    .expect("Executed phase always carries executed_at")
                    .elapsed();
                RunFailure::executed(message, executed_fallback_usage(mem_bytes, elapsed, None))
            }
        }
    })?;

    let executed_at = child.executed_at();

    let stdin_pipe = if stdin.is_some() {
        child.stdin().take()
    } else {
        None
    };
    if stdin.is_some() && stdin_pipe.is_none() {
        *child_retirement = child.kill_and_wait();
        return Err(RunFailure::executed(
            "runsc stdin pipe unavailable",
            executed_fallback_usage(mem_bytes, executed_at.elapsed(), None),
        ));
    }
    let stdin_th = stdin.zip(stdin_pipe).map(|(source, mut si)| {
        std::thread::spawn(move || match source {
            StdinSource::Bytes(bytes) => si.write_all(&bytes),
            StdinSource::File(mut file) => std::io::copy(&mut file, &mut si).map(|_| ()),
        })
    });

    let pid = child.id();

    let (Some(mut out), Some(mut err)) = (child.stdout().take(), child.stderr().take()) else {
        *child_retirement = child.kill_and_wait();
        if let Some(t) = stdin_th {
            let _ = t.join();
        }
        return Err(RunFailure::executed(
            "runsc output pipe unavailable",
            executed_fallback_usage(mem_bytes, executed_at.elapsed(), None),
        ));
    };
    let stdout_output = output.clone();
    let stdout_redaction = redaction.clone();
    let th_out = std::thread::spawn(move || match (stdout_mode, stdout_output) {
        (StdoutMode::CappedHead, Some(output)) => drain_capped_streaming(
            &mut out,
            SANDBOX_CAPTURE_BOUND,
            SandboxOutputStream::Stdout,
            Some(&output),
            &stdout_redaction,
        ),
        (StdoutMode::CappedHead, None) => drain_capped_streaming(
            &mut out,
            SANDBOX_CAPTURE_BOUND,
            SandboxOutputStream::Stdout,
            None,
            &stdout_redaction,
        ),
        (StdoutMode::StreamToFile { bound }, _) => {
            let (head, truncated) = drain_to_temp_file(&mut out, bound);
            (head, truncated, None)
        }
    });
    let th_err = std::thread::spawn(move || match output {
        Some(output) => {
            let (head, _, error) = drain_capped_streaming(
                &mut err,
                SANDBOX_CAPTURE_BOUND,
                SandboxOutputStream::Stderr,
                Some(&output),
                &redaction,
            );
            (head, error)
        }
        None => {
            let (head, _, error) = drain_capped_streaming(
                &mut err,
                SANDBOX_CAPTURE_BOUND,
                SandboxOutputStream::Stderr,
                None,
                &redaction,
            );
            (head, error)
        }
    });

    let timed_out;
    let mut last_cpu: Option<u64> = None;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                timed_out = child.watchdog_deadline_expired();
                *child_retirement = DirectChildRetirement::Reaped;
                break status.code();
            }
            Ok(None) => {}
            Err(error) => {
                *child_retirement = child.kill_and_wait();
                let _ = th_out.join();
                let _ = th_err.join();
                if let Some(t) = stdin_th {
                    let _ = t.join();
                }
                return Err(RunFailure::executed(
                    format!("wait runsc: {error}"),
                    executed_fallback_usage(mem_bytes, executed_at.elapsed(), last_cpu),
                ));
            }
        }
        if let Some(c) = read_proc_cpu_seconds(pid) {
            last_cpu = Some(c);
        }
        let cancelled = cancellation.load(Ordering::Acquire);
        if cancelled || executed_at.elapsed() >= timeout {
            let mut kill_cmd = Command::new(bin);
            if apply_runsc_invocation_policy(&mut kill_cmd, bin, mode).is_ok() {
                let _ = kill_cmd.arg("kill").arg(container_id).arg("KILL").output();
            }
            *child_retirement = child.kill_and_wait();
            timed_out = !cancelled;
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let wall = executed_at.elapsed();

    let stdout_result = th_out.join();
    let stderr_result = th_err.join();
    let stdin_result = stdin_th.map(std::thread::JoinHandle::join);

    let (stdout, stdout_truncated, stdout_error) = stdout_result.map_err(|_| {
        RunFailure::executed(
            "runsc stdout drain thread panicked",
            executed_fallback_usage(mem_bytes, wall, last_cpu),
        )
    })?;
    let (stderr, stderr_error) = stderr_result.map_err(|_| {
        RunFailure::executed(
            "runsc stderr drain thread panicked",
            executed_fallback_usage(mem_bytes, wall, last_cpu),
        )
    })?;
    if let Some(write_result) = stdin_result {
        let write_result = write_result.map_err(|_| {
            RunFailure::executed(
                "runsc stdin writer thread panicked",
                executed_fallback_usage(mem_bytes, wall, last_cpu),
            )
        })?;
        if exit == Some(0) {
            write_result.map_err(|e| {
                RunFailure::executed(
                    format!("write runsc stdin: {e}"),
                    executed_fallback_usage(mem_bytes, wall, last_cpu),
                )
            })?;
        }
    }

    Ok(RunscOutcome {
        exit,
        timed_out,
        stdout,
        stdout_truncated,
        stderr,
        wall,
        cpu_seconds: last_cpu,
        stream_error: stdout_error.or(stderr_error).or_else(|| {
            (has_streaming_output && cancellation.load(Ordering::Acquire))
                .then(|| "sandbox execution cancelled by durable log consumer".into())
        }),
    })
}

pub(super) enum StdoutMode {
    CappedHead,
    StreamToFile { bound: usize },
}

pub(super) struct RunCaptureOptions<'a> {
    pub(super) stdin: Option<StdinSource>,
    pub(super) stdout_mode: StdoutMode,
    pub(super) cancellation: &'a AtomicBool,
    pub(super) redaction: RedactionPlan,
    pub(super) output: Option<StreamingOutput>,
}

pub(super) enum StdinSource {
    Bytes(Vec<u8>),
    File(std::fs::File),
}

#[derive(Clone)]
pub(super) struct StreamingOutput {
    pub(super) sink: Arc<dyn SandboxOutputSink>,
}

const TOTAL_LOG_TRUNCATION_MARKER: &[u8] =
    b"\n[myelin: total job log byte limit reached; output truncated]\n";
const SANDBOX_TOTAL_LOG_CAPTURE_BOUND: usize = 2 * SANDBOX_CAPTURE_BOUND;

#[derive(Debug)]
struct TotalLogCaptureState {
    payload_bytes: usize,
    captured_bytes: usize,
    stopped: bool,
}

struct TotalLogCappedOutput {
    sink: Arc<dyn SandboxOutputSink>,
    total_limit: usize,
    state: Mutex<TotalLogCaptureState>,
}

impl TotalLogCappedOutput {
    pub(super) fn new(sink: Arc<dyn SandboxOutputSink>) -> Self {
        Self::with_limit(sink, SANDBOX_TOTAL_LOG_CAPTURE_BOUND)
    }

    fn with_limit(sink: Arc<dyn SandboxOutputSink>, total_limit: usize) -> Self {
        assert!(
            total_limit >= TOTAL_LOG_TRUNCATION_MARKER.len(),
            "the total log limit must have room for its truncation marker"
        );
        Self {
            sink,
            total_limit,
            state: Mutex::new(TotalLogCaptureState {
                payload_bytes: 0,
                captured_bytes: 0,
                stopped: false,
            }),
        }
    }

    #[cfg(test)]
    fn captured_bytes(&self) -> usize {
        self.state.lock().unwrap().captured_bytes
    }
}

impl SandboxOutputSink for TotalLogCappedOutput {
    fn emit(&self, stream: SandboxOutputStream, frame: &[u8]) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| {
            "total job log capture state poisoned; refusing further output".to_string()
        })?;
        if state.stopped || frame.is_empty() {
            return Ok(());
        }

        let payload_limit = self.total_limit - TOTAL_LOG_TRUNCATION_MARKER.len();
        let remaining = payload_limit.saturating_sub(state.payload_bytes);
        let take = remaining.min(frame.len());
        if take > 0 {
            if let Err(error) = self.sink.emit(stream, &frame[..take]) {
                state.stopped = true;
                return Err(error);
            }
            state.payload_bytes += take;
            state.captured_bytes += take;
        }

        if take < frame.len() {
            state.stopped = true;
            self.sink.emit(stream, TOTAL_LOG_TRUNCATION_MARKER)?;
            state.captured_bytes += TOTAL_LOG_TRUNCATION_MARKER.len();
        }
        Ok(())
    }
}

pub(super) fn cap_total_job_output(
    output: Arc<dyn SandboxOutputSink>,
) -> Arc<dyn SandboxOutputSink> {
    Arc::new(TotalLogCappedOutput::new(output))
}

fn drain_capped_streaming<R: Read>(
    mut reader: R,
    limit: usize,
    stream: SandboxOutputStream,
    output: Option<&StreamingOutput>,
    redaction: &RedactionPlan,
) -> (Vec<u8>, bool, Option<String>) {
    let mut head = Vec::new();
    let mut truncated = false;
    let mut first_output_error = None;
    let mut chunk = [0u8; 64 * 1024];
    let mut redactor = redaction.streaming();
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let redacted = redactor.push(&chunk[..n]);
                append_capped(&mut head, &redacted, limit, &mut truncated);
                if first_output_error.is_none() && !redacted.is_empty() {
                    if let Some(output) = output {
                        if let Err(error) = output.sink.emit(stream, &redacted) {
                            first_output_error = Some(error);
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                truncated = true;
                if first_output_error.is_none() {
                    first_output_error = Some(format!("read guest output: {error}"));
                }
                break;
            }
        }
    }
    let redacted = redactor.finish();
    append_capped(&mut head, &redacted, limit, &mut truncated);
    if first_output_error.is_none() && !redacted.is_empty() {
        if let Some(output) = output {
            if let Err(error) = output.sink.emit(stream, &redacted) {
                first_output_error = Some(error);
            }
        }
    }
    (head, truncated, first_output_error)
}

fn append_capped(head: &mut Vec<u8>, bytes: &[u8], limit: usize, truncated: &mut bool) {
    let remaining = limit.saturating_sub(head.len());
    let take = remaining.min(bytes.len());
    head.extend_from_slice(&bytes[..take]);
    *truncated |= take < bytes.len();
}

fn drain_to_temp_file<R: Read>(mut r: R, cap: usize) -> (Vec<u8>, bool) {
    use std::os::unix::fs::OpenOptionsExt;
    let path = std::env::temp_dir().join(format!(
        "myelin-gitwire-stdout-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = std::fs::remove_file(&path);
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
    {
        Ok(f) => f,
        Err(_) => return drain_capped(&mut r, cap),
    };
    if std::fs::remove_file(&path).is_err() {
        return drain_capped(&mut r, cap);
    }
    let mut written: usize = 0;
    let mut truncated = false;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if written < cap {
                    let take = (cap - written).min(n);
                    if file.write_all(&chunk[..take]).is_err() {
                        truncated = true;
                        written = cap;
                    } else {
                        written += take;
                        if take < n {
                            truncated = true;
                        }
                    }
                } else {
                    truncated = true;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                truncated = true;
                break;
            }
        }
    }
    let _ = file.flush();
    let head = match file.seek(std::io::SeekFrom::Start(0)).and_then(|_| {
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        Ok(buf)
    }) {
        Ok(bytes) => bytes,
        Err(_) => {
            truncated = true;
            Vec::new()
        }
    };
    (head, truncated)
}

fn executed_fallback_usage(
    mem_bytes: u64,
    elapsed: Duration,
    cpu_seconds: Option<u64>,
) -> ResourceUsage {
    let wall_secs_ceil = (elapsed.as_secs() + u64::from(elapsed.subsec_nanos() > 0)).max(1);
    let cpu_seconds = cpu_seconds.filter(|c| *c > 0).unwrap_or(wall_secs_ceil);
    ResourceUsage {
        cpu_seconds,
        mem_byte_seconds: mem_bytes.saturating_mul(wall_secs_ceil),
    }
}

pub(super) fn build_result(
    spec: &JobSpec,
    o: &RunscOutcome,
    redaction: &RedactionPlan,
) -> SandboxResult {
    let stdout = redaction.redact(&o.stdout);
    let mut stderr = redaction.redact(&o.stderr);
    if stderr.len() > SANDBOX_CAPTURE_BOUND {
        stderr.truncate(SANDBOX_CAPTURE_BOUND);
    }
    let exit_code = if o.timed_out { None } else { o.exit };

    let wall_secs_ceil = o.wall.as_secs() + u64::from(o.wall.subsec_nanos() > 0);
    let cpu_seconds = o.cpu_seconds.filter(|c| *c > 0).unwrap_or(wall_secs_ceil);
    let mem_byte_seconds = spec.limits.mem_bytes.saturating_mul(wall_secs_ceil);

    SandboxResult {
        exit_code,
        timed_out: o.timed_out,
        usage: ResourceUsage {
            cpu_seconds,
            mem_byte_seconds,
        },
        stdout,
        stderr,
    }
}

pub(super) struct SpawnedRunsc {
    pub(super) bin: &'static Path,
    pub(super) container_id: String,
    pub(super) mode: RunscInvocationMode,
}
impl RunscChild for SpawnedRunsc {
    fn kill(&mut self) -> Result<(), String> {
        delete_container(self.bin, &self.container_id, self.mode);
        Ok(())
    }
    fn wait(&mut self) -> Result<i32, String> {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Read;

    use std::sync::Arc;

    use crate::gvisor::test_fixtures::*;
    use crate::redaction::RedactionPlan;
    use crate::{SandboxOutputSink, SandboxOutputStream, SANDBOX_CAPTURE_BOUND};

    #[test]
    fn streaming_drain_keeps_only_the_head_but_forwards_chunks_to_the_job_budget() {
        let input: Vec<u8> = (0..(3 * 64 * 1024 + 17))
            .map(|offset| (offset % 251) as u8)
            .collect();
        let sink = Arc::new(RecordingOutput::default());
        let capped: Arc<dyn SandboxOutputSink> = Arc::new(TotalLogCappedOutput::new(sink.clone()));
        let output = StreamingOutput { sink: capped };
        let redaction = RedactionPlan::none();

        let (head, truncated, error) = drain_capped_streaming(
            std::io::Cursor::new(&input),
            1024,
            SandboxOutputStream::Stdout,
            Some(&output),
            &redaction,
        );

        assert_eq!(error, None);
        assert!(truncated);
        assert_eq!(head, input[..1024]);
        assert_eq!(
            *sink.bytes.lock().unwrap(),
            input,
            "bytes beyond the diagnostic head still reach the shared job budget when under it"
        );
    }

    #[test]
    fn streaming_drain_masks_an_injected_value_split_across_pipe_reads() {
        let sink = Arc::new(RecordingOutput::default());
        let output = StreamingOutput { sink: sink.clone() };
        let redaction = RedactionPlan::for_needles([b"split-secret".to_vec()]).unwrap();
        let reader = std::io::Cursor::new(b"before split-".as_slice())
            .chain(std::io::Cursor::new(b"secret after".as_slice()));

        let (_head, _truncated, error) = drain_capped_streaming(
            reader,
            1024,
            SandboxOutputStream::Stdout,
            Some(&output),
            &redaction,
        );

        assert_eq!(error, None);
        assert!(!sink
            .bytes
            .lock()
            .unwrap()
            .windows(b"split-secret".len())
            .any(|window| window == b"split-secret"));
    }

    #[test]
    fn result_head_redacts_before_truncating_a_boundary_straddling_secret() {
        let secret = b"BOUNDARY-STRADDLING-SECRET";
        let prefix_len = secret.len() / 2;
        let mut input = vec![b'a'; SANDBOX_CAPTURE_BOUND - prefix_len];
        input.extend_from_slice(secret);
        input.extend(std::iter::repeat_n(b'z', SANDBOX_CAPTURE_BOUND));
        let redaction = RedactionPlan::for_needles([secret.to_vec()]).unwrap();

        let (head, truncated, error) = drain_capped_streaming(
            std::io::Cursor::new(input),
            SANDBOX_CAPTURE_BOUND,
            SandboxOutputStream::Stdout,
            None,
            &redaction,
        );

        assert_eq!(error, None);
        assert!(truncated);
        assert!(!head.windows(secret.len()).any(|window| window == secret));
        assert!(!head[..].ends_with(&secret[..prefix_len]));
    }

    #[test]
    fn streaming_capture_total_log_cap_truncates_with_marker_and_stops_growth() {
        let payload_limit = 1024;
        let total_limit = payload_limit + TOTAL_LOG_TRUNCATION_MARKER.len();
        let sink = Arc::new(RecordingOutput::default());
        let capped = Arc::new(TotalLogCappedOutput::with_limit(sink.clone(), total_limit));
        let output = StreamingOutput {
            sink: capped.clone(),
        };
        let redaction = RedactionPlan::none();
        let input = vec![b'x'; 256 * 1024];

        let (_head, truncated, error) = drain_capped_streaming(
            std::io::Cursor::new(&input),
            128,
            SandboxOutputStream::Stdout,
            Some(&output),
            &redaction,
        );
        assert_eq!(error, None);
        assert!(truncated, "the diagnostic head is also truncated");

        let captured_before_late_frame = sink.bytes.lock().unwrap().clone();
        capped
            .emit(SandboxOutputStream::Stderr, b"late bytes must be discarded")
            .unwrap();
        let captured = sink.bytes.lock().unwrap().clone();
        assert_eq!(captured, captured_before_late_frame);
        assert_eq!(captured.len(), total_limit);
        assert_eq!(&captured[..payload_limit], &input[..payload_limit]);
        assert!(captured.ends_with(TOTAL_LOG_TRUNCATION_MARKER));
        assert_eq!(capped.captured_bytes(), total_limit);
    }

    #[test]
    fn total_log_cap_is_shared_across_streams_and_under_ceiling_is_unchanged() {
        let sink = Arc::new(RecordingOutput::default());
        let total_limit = TOTAL_LOG_TRUNCATION_MARKER.len() + 64;
        let capped = TotalLogCappedOutput::with_limit(sink.clone(), total_limit);
        capped
            .emit(SandboxOutputStream::Stdout, b"ordinary stdout\n")
            .unwrap();
        capped
            .emit(SandboxOutputStream::Stderr, b"ordinary stderr\n")
            .unwrap();
        assert_eq!(
            *sink.bytes.lock().unwrap(),
            b"ordinary stdout\nordinary stderr\n",
            "combined output below the payload ceiling must be byte-identical"
        );

        let sink = Arc::new(RecordingOutput::default());
        let capped =
            TotalLogCappedOutput::with_limit(sink.clone(), TOTAL_LOG_TRUNCATION_MARKER.len() + 8);
        capped.emit(SandboxOutputStream::Stdout, b"123456").unwrap();
        capped.emit(SandboxOutputStream::Stderr, b"abcdef").unwrap();
        let captured = sink.bytes.lock().unwrap().clone();
        assert_eq!(&captured[..8], b"123456ab");
        assert!(captured.ends_with(TOTAL_LOG_TRUNCATION_MARKER));
        assert_eq!(
            captured.len(),
            TOTAL_LOG_TRUNCATION_MARKER.len() + 8,
            "stdout and stderr must consume one exact shared total-byte budget"
        );
    }

    #[test]
    fn build_result_masks_needles_in_both_streams() {
        let s = spec(vec![]);
        let needle = [b"AK".as_slice(), b"IAsecret"].concat();
        let stdout = [b"deploying with ".as_slice(), needle.as_slice(), b" now"].concat();
        let stderr = [b"error: ".as_slice(), needle.as_slice(), b" invalid"].concat();
        let plan = RedactionPlan::for_needles([needle.clone()]).unwrap();
        let o = outcome(&stdout, &stderr);
        let res = build_result(&s, &o, &plan);
        assert!(res.stdout.starts_with(b"deploying with "));
        assert!(res.stdout.ends_with(b" now"));
        assert!(res.stderr.starts_with(b"error: "));
        assert!(res.stderr.ends_with(b" invalid"));
        assert!(!res
            .stdout
            .windows(needle.len())
            .any(|window| window == needle));
        assert!(!res
            .stderr
            .windows(needle.len())
            .any(|window| window == needle));
    }

    #[test]
    fn injected_secret_value_is_absent_from_sandbox_result_when_workload_prints_it() {
        let mut s = spec(vec![]);
        s.secret_refs = vec![crate::SecretRef {
            name: "DEPLOY_TOKEN".into(),
            handle: "opaque:deploy".into(),
        }];
        let material = ["printed", "-secret-material"].concat();
        let s = s
            .with_resolved_secrets(vec![crate::ResolvedSecretEnv::new(
                "DEPLOY_TOKEN",
                material.clone(),
            )])
            .expect("binding and plan are derived together");
        let stdout = format!("stdout:{material}");
        let stderr = format!("stderr:{material}");
        let outcome = outcome(stdout.as_bytes(), stderr.as_bytes());

        let result = build_result(&s, &outcome, s.resolved_secrets().redaction_plan());

        assert!(result.stdout.starts_with(b"stdout:"));
        assert!(result.stderr.starts_with(b"stderr:"));
        assert!(!result
            .stdout
            .windows(material.len())
            .any(|window| window == material.as_bytes()));
        assert!(!result
            .stderr
            .windows(material.len())
            .any(|window| window == material.as_bytes()));
        assert!(!format!("{result:?}").contains(&material));
    }

    #[test]
    fn build_result_empty_plan_is_byte_identity() {
        let s = spec(vec![]);
        let o = outcome(b"ordinary build log line", b"warning: deprecated");
        let res = build_result(&s, &o, &RedactionPlan::none());
        assert_eq!(res.stdout, b"ordinary build log line".to_vec());
        assert_eq!(res.stderr, b"warning: deprecated".to_vec());
    }

    #[test]
    fn drain_to_temp_file_streams_whole_under_cap_and_flags_over_cap() {
        let big = vec![0xABu8; 1024 * 1024];
        let (out, truncated) = drain_to_temp_file(&big[..], 4 * 1024 * 1024);
        assert_eq!(
            out.len(),
            big.len(),
            "a real-size pack under the cap comes through WHOLE"
        );
        assert_eq!(
            out, big,
            "the bytes are byte-identical (no corruption via the temp file)"
        );
        assert!(!truncated, "within the cap ⇒ not truncated");

        let (head, over) = drain_to_temp_file(&big[..], 64 * 1024);
        assert_eq!(
            head.len(),
            64 * 1024,
            "over the cap ⇒ exactly the cap bytes are kept"
        );
        assert!(
            over,
            "over the cap ⇒ truncated flag set (the wire seam then refuses loudly)"
        );
    }

    #[test]
    fn drain_to_temp_file_marks_a_read_fault_as_incomplete() {
        struct FaultAfterPrefix(Option<&'static [u8]>);

        impl Read for FaultAfterPrefix {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if let Some(prefix) = self.0.take() {
                    buf[..prefix.len()].copy_from_slice(prefix);
                    Ok(prefix.len())
                } else {
                    Err(std::io::Error::other("injected wire read fault"))
                }
            }
        }

        let (head, incomplete) = drain_to_temp_file(FaultAfterPrefix(Some(b"partial-pack")), 1024);

        assert_eq!(head, b"partial-pack");
        assert!(incomplete);
    }
}

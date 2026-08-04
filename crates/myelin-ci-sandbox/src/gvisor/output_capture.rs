//! Spawning `runsc` and capturing its output: the bounded stdout/stderr drains, the total-log cap
//! over a streaming sink, and the [`SandboxResult`] built from the raw outcome.

use super::*;
use crate::launch_gate::{DirectChildRetirement, SandboxCommand, SpawnPhase};
use crate::redaction::RedactionPlan;
use crate::user_namespace::RunscInvocationMode;
use crate::{
    drain_capped, JobSpec,
    LaunchPermit, ResourceUsage, SandboxOutputSink, SandboxOutputStream,
    SandboxResult, SANDBOX_CAPTURE_BOUND,
};
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(super) static NEVER_CANCELLED: AtomicBool = AtomicBool::new(false);

/// The raw outcome of running `runsc run` to completion (or to a timeout-kill) — consumed by
/// [`build_result`] into a [`SandboxResult`].
pub(super) struct RunscOutcome {
    /// The `runsc` child's REAL exit status = the container process's actual exit code. `None` if the
    /// container was timeout-killed (no trustworthy code — never fabricated).
    pub(super) exit: Option<i32>,
    /// True iff the wall-clock `timeout_secs` ceiling fired and the whole container was killed.
    pub(super) timed_out: bool,
    /// The container's REAL piped stdout (the runtime's fd, not in-container framing). Bounded by the
    /// stream's [`StdoutMode`] (the 256 KiB head bound for CI/agent logs; the GENEROUS git-wire cap,
    /// disk-streamed, for the wire path).
    pub(super) stdout: Vec<u8>,
    /// True iff the stdout stream exceeded its [`StdoutMode`] bound (head-truncated). For the CI/agent
    /// path this is benign (logs are head-captured by design); for the git-wire path it is FATAL — a
    /// truncated packfile fails the client's `index-pack` with "early EOF", so the wire seam REFUSES it
    /// loudly (never returns a silently-truncated pack). See [`run_git_wire_container`].
    pub(super) stdout_truncated: bool,
    /// The container's REAL piped stderr (always 256 KiB head-bounded — it is error text, not payload).
    pub(super) stderr: Vec<u8>,
    /// Wall-clock duration the container ran.
    pub(super) wall: Duration,
    /// Host-side CPU-seconds of the `runsc` process (utime+stime from `/proc`), if readable.
    pub(super) cpu_seconds: Option<u64>,
    /// Callback/read/cancellation failure observed after the container started.
    pub(super) stream_error: Option<String>,
}

/// A phase-tagged [`run_and_capture`] failure — CT-007 vertical-slice step 2b's gvisor.rs-
/// integration planning found a real, pre-existing gap this closes: `launch_with` used to
/// propagate a bare `String` from this function and return early WITHOUT ever calling
/// `hooks.release_unused` or `hooks.settle_completed`, silently leaking the job's cost/capacity
/// reservation on every failure path — regardless of whether the sandbox had actually spawned
/// and consumed real host resources by the time it failed. The four variants (mirroring
/// [`crate::launch_gate::SpawnPhase`]'s own four-way distinction from the durable-launch-commit
/// boundary) tell the caller exactly which of `release_unused` / `DurableOutcomeUnknown` /
/// `settle_completed(zero)` / `settle_completed(usage)` is correct — `Executed` makes its usage
/// mandatory (not an `Option`), so an "executed but no usage was ever computed" state cannot be
/// constructed at all. This prevents a MISSING usage, not a zero one — a caller can still
/// construct `Executed` with a genuinely zero `ResourceUsage`; every real call site computes it via
/// `executed_fallback_usage`, which floors elapsed wall-time at 1 second precisely so that never
/// happens in practice.
// CT-007 slice 5b.3-6c: `pub(crate)` so the closed capsule workload transition
// (`PreparedCheckoutRuntime::run_retained_workload`, in the child `checkout_runtime` module) can carry
// it in its owned `RetainedWorkloadOutcome` — the parent module cannot reach a private child method, so
// the accessor must be `pub(crate)`, and its return type therefore must be too.
#[derive(Debug)]
pub(crate) enum RunFailure {
    /// No durable launch CAS committed. Safe to release the reservation at zero cost.
    Uncommitted { message: String },
    /// The durable launch CAS returned an error, but whether it actually committed durably is
    /// UNKNOWN. Neither release nor settle is safe; the caller must defer to reconciliation.
    CommitOutcomeUnknown { message: String },
    /// The durable launch CAS committed, but the runtime never got to exec. The reservation's
    /// cost is real (the commit itself is durably accounted) even though no workload ran.
    CommittedButNotExecuted { message: String },
    /// The runtime was released to exec (or genuinely started, for an unfenced spawn) by the
    /// time this failure occurred. `usage` is the conservative fallback accounting — mandatory
    /// (an "executed but no usage was ever computed" state cannot be constructed), computed by
    /// every real call site via `executed_fallback_usage`'s 1-second wall-time floor so a job
    /// engineered to fail exactly after exec cannot run for free in practice.
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

    fn commit_outcome_unknown(message: impl Into<String>) -> Self {
        Self::CommitOutcomeUnknown {
            message: message.into(),
        }
    }

    fn committed_but_not_executed(message: impl Into<String>) -> Self {
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

/// Spawn the REAL `runsc` container (`runsc --rootless --network=none run -bundle <dir> <cid>`) — THE
/// one legitimate runtime-spawn site (the `no-host-exec` named exclusion; the mechanism that CREATES
/// the userspace-kernel boundary, not a bypass) — drain its stdout/stderr on dedicated threads (so a
/// chatty container cannot fill a pipe buffer and deadlock), and wait at most `timeout`. On expiry the
/// WHOLE CONTAINER is killed (`runsc kill <cid> KILL` then the child) and `timed_out` is set. The exit
/// code is the `runsc` child's REAL `ExitStatus.code()` — the container process's actual exit, never
/// parsed from container output (the structural reason gVisor needs no forge defense).
///
/// CT-007 slice 3, piece 7b: `cgroup` is BORROWED, not created here — ownership belongs to the
/// caller, which must run checked teardown ([`finalize_runtime`]) once this returns, using the
/// SAME cgroup value. This function never calls `cgroup.cleanup()`/`quiesce()` itself. It always
/// returns a [`DirectChildRetirement`] alongside its primary result (`Ok` or `Err`) — every early
/// return before `sandbox_command.spawn()` succeeds implies [`DirectChildRetirement::NoChildReturned`];
/// every return after implies whatever the real (no longer discarded) `wait()` outcome was.
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
    // A fenced command's stdin is ALWAYS a pipe (the gate script itself reads its release line from
    // it) regardless of whether this caller has real bytes to feed the guest afterward — so a
    // caller that does (the git-wire request body) must explicitly ask for that SAME pipe back once
    // the gate releases, rather than have it closed for EOF the instant release happens (task #33's
    // root cause: this call was previously missing entirely, so every fenced launch closed the pipe
    // unconditionally and `child.stdin()` was always `None` by the time this function reached it).
    if fenced && stdin.is_some() {
        sandbox_command.return_stdin_to_caller_after_gate();
    }
    {
        let cmd = sandbox_command.command_mut();
        apply_runsc_invocation_policy(cmd, bin, mode).map_err(RunFailure::uncommitted)?;
        cmd.arg("--network=none")
            .arg("run")
            .arg("-bundle")
            .arg(bundle)
            .arg(container_id)
            // CT-006a: the git-wire path pipes the stateless-rpc request body in; CI/agent jobs get no
            // stdin (`null`). The bytes are already bounded by [`WIRE_STDIN_BOUND`] before we get here.
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !fenced {
            cmd.stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        }
        // Place the runsc child (and the sentry/gofer tree it forks) into the memory cgroup at birth.
        cgroup.place_child(cmd).map_err(|e| {
            RunFailure::uncommitted(format!("bind runsc into the memory cgroup: {e}"))
        })?;
    }
    if fenced {
        let kill_file = cgroup.kill_file().map_err(|error| {
            RunFailure::uncommitted(format!("open runsc cgroup kill switch: {error}"))
        })?;
        sandbox_command.kill_cgroup_on_liveness_loss(kill_file);
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

    // From here on, `sandbox_command.spawn()` has succeeded — per its own contract, the runtime
    // has ALREADY been released to exec (the fenced case's gate release + ownership.release()
    // both succeeded before `Ok(SandboxChild)` is ever returned). Every failure below is
    // therefore `Executed`: the sandbox may already be consuming real host resources, and must
    // never be charged zero. `executed_at` is the TRUE release/spawn moment `SandboxChild`
    // captured (immediately before the gate write for a fenced launch, immediately before
    // `Command::spawn` for an unfenced one) — never a LATER `Instant::now()` taken here, which
    // would silently exclude real gate-release/pipe-setup execution time from every fallback
    // computation below, AND from the timeout deadline and successful `wall` duration further on.
    let executed_at = child.executed_at();

    // CT-006a: feed the bounded request body to the container's stdin on a DEDICATED thread (so a
    // large body + a slow in-guest reader cannot deadlock against our stdout/stderr drains), then drop
    // the handle to deliver EOF (the stateless-rpc request terminator). None ⇒ stdin was `null`.
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
        std::thread::spawn(move || {
            let result = match source {
                StdinSource::Bytes(bytes) => si.write_all(&bytes),
                StdinSource::File(mut file) => std::io::copy(&mut file, &mut si).map(|_| ()),
            };
            // `si` drops here ⇒ the write end closes ⇒ the guest `git` sees EOF on its request body.
            result
        })
    });

    let pid = child.id();

    // Drain both pipes on threads so a chatty container cannot fill a pipe buffer and deadlock.
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
    // stdout draining depends on the stream's mode (CT-006c):
    //   - `CappedHead` (CI/agent logs): cap at SANDBOX_CAPTURE_BOUND (256 KiB head capture) + DISCARD the
    //     rest to EOF — bounds host memory under a runaway container, byte-unchanged from CT-002c.
    //   - `StreamToFile` (the git wire): stream straight to a host TEMP FILE under a GENEROUS cap so a
    //     real-size packfile (megabytes, not 256 KiB) comes through WHOLE while host RAM stays one chunk
    //     (the bytes land on disk, not in a growing Vec). Over the generous cap ⇒ `truncated` (the wire
    //     seam then REFUSES loudly — never a silently-truncated pack). Both keep reading past the bound
    //     so the container never blocks on a full pipe (no deadlock that would defeat the timeout).
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
    // stderr is ALWAYS the 256 KiB head bound — it is error text folded into a message, never payload.
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
                // `try_wait()` itself just returned a real exit status — this IS the confirmed reap.
                *child_retirement = DirectChildRetirement::Reaped;
                break status.code();
            }
            Ok(None) => {}
            Err(error) => {
                // Kill/reap FIRST so the pipes hit EOF, THEN join every thread that was reading
                // them — no thread may outlive this run, even on a wait-syscall failure (a bare
                // early return here would leak the stdin/stdout/stderr threads, still blocked on
                // pipes from a child nothing ever killed).
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
            // Wall-clock ceiling hit: whole-CONTAINER kill (SIGKILL the container's PID1 via the
            // runtime), then reap the `runsc` child process so the pipes hit EOF.
            // The SAME `mode` already passed `apply_runsc_invocation_policy` once, at this exact
            // container's spawn earlier in this function call — the policy this reads from is a
            // process-lifetime `OnceLock` that cannot have changed since, so failure here is not
            // reachable in practice. Best-effort regardless: if it somehow did fail, skip the
            // `runsc kill` (nothing safe to send it with) but still reap the host-side child below.
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

    // The child has exited/been-killed ⇒ the pipes hit EOF ⇒ the drain threads finish.
    let stdout_result = th_out.join();
    let stderr_result = th_err.join();
    // The writer thread has finished (the child read its request body, or it exited and the write
    // EPIPE'd — either way `write_all` returned). Join so no thread outlives the run.
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
        // A child that failed or was killed may legitimately close stdin early; its primary outcome
        // remains authoritative. A successful child, however, must have received the full bounded
        // request body or the Git wire exchange is incomplete.
        if exit == Some(0) {
            write_result.map_err(|e| {
                RunFailure::executed(
                    format!("write runsc stdin: {e}"),
                    executed_fallback_usage(mem_bytes, wall, last_cpu),
                )
            })?;
        }
    }

    // The container + its sentry/gofer tree are gone. Cgroup teardown is now the CALLER's
    // responsibility (it owns `cgroup`, borrowed here) — see [`finalize_runtime`].

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

/// How the container's stdout is drained (CT-006c). The non-wire (CI/agent) path keeps the byte-unchanged
/// 256 KiB head capture; the git-wire path streams to a host temp file under a generous cap so a
/// real-size packfile survives whole while host RAM stays bounded to one chunk.
pub(super) enum StdoutMode {
    /// CI/agent logs: head-capture the first [`SANDBOX_CAPTURE_BOUND`] bytes in RAM, discard the rest.
    CappedHead,
    /// The git wire: stream straight to a host temp file under `bound` bytes (host RAM stays one 64 KiB
    /// chunk regardless of pack size), then materialize back. Over `bound` ⇒ the returned `truncated`
    /// flag is set and the wire seam refuses loudly.
    StreamToFile { bound: usize },
}

pub(super) struct RunCaptureOptions<'a> {
    pub(super) stdin: Option<StdinSource>,
    pub(super) stdout_mode: StdoutMode,
    pub(super) cancellation: &'a AtomicBool,
    pub(super) redaction: RedactionPlan,
    pub(super) output: Option<StreamingOutput>,
}

/// The one-shot source `run_and_capture` feeds to the guest's stdin (CT-007 slice 5b.2). `Bytes` is
/// the pre-existing git-wire shape (the bounded request body, already resident in memory). `File` is
/// new: the checkout-preparation runtime's prefetched pack lives in a bounded HOST TEMP FILE (never
/// materialized as a second in-memory `Vec` alongside the one `run_git_wire_container` already reads
/// it into) — the writer thread `io::copy`s it straight into the gated pipe. Neither variant is
/// `Clone`; each is consumed exactly once by the writer thread `run_and_capture_impl` spawns.
#[allow(dead_code)]
pub(super) enum StdinSource {
    Bytes(Vec<u8>),
    File(std::fs::File),
}

#[derive(Clone)]
pub(super) struct StreamingOutput {
    pub(super) sink: Arc<dyn SandboxOutputSink>,
}

/// One marker is stored inside (not in addition to) the total per-job log ceiling. Reserving its
/// fixed size means an adversarial stream can never make durable capture exceed the exact bound.
const TOTAL_LOG_TRUNCATION_MARKER: &[u8] =
    b"\n[myelin: total job log byte limit reached; output truncated]\n";
const SANDBOX_TOTAL_LOG_CAPTURE_BOUND: usize = 2 * SANDBOX_CAPTURE_BOUND;

#[derive(Debug)]
struct TotalLogCaptureState {
    payload_bytes: usize,
    captured_bytes: usize,
    stopped: bool,
}

/// Per-job choke point in front of the durable output sink. Every stdout/stderr frame and every
/// checkout/workload phase for one job shares this single state, so a tenant cannot multiply the
/// ceiling by alternating streams or phases.
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
            // Mark stopped before invoking the external sink: even a marker-write failure cannot
            // reopen capture or let later frames grow storage without bound.
            state.stopped = true;
            self.sink.emit(stream, TOTAL_LOG_TRUNCATION_MARKER)?;
            state.captured_bytes += TOTAL_LOG_TRUNCATION_MARKER.len();
        }
        Ok(())
    }
}

pub(super) fn cap_total_job_output(output: Arc<dyn SandboxOutputSink>) -> Arc<dyn SandboxOutputSink> {
    Arc::new(TotalLogCappedOutput::new(output))
}

/// Drain a complete guest stream while retaining only its bounded diagnostic head and forwarding
/// chunks to the job-wide, total-byte-capped durable-output callback.
///
/// A callback failure is remembered, but the pipe is still drained to EOF so the guest cannot
/// deadlock behind a full pipe and defeat timeout/teardown. Redaction is applied before the callback,
/// with a per-stream carry buffer so a secret split across arbitrary pipe reads cannot cross into
/// durable output.
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

/// **Drain a child stream straight to a host TEMP FILE under a generous byte cap (the git-wire path,
/// CT-006c).** Host MEMORY stays bounded to ONE 64 KiB chunk regardless of how large the packfile is —
/// the bytes are written to disk as they arrive, NOT buffered in a growing Vec. Keeps reading past the
/// cap (draining + discarding) so the container never blocks on a full pipe (no deadlock that would
/// defeat the timeout). Returns the materialized head (≤ `cap` bytes, read back from the temp file) and
/// whether the cap was exceeded or the stream could not be read through EOF (`truncated`). The temp
/// file is removed before returning (no leak).
///
/// NOTE (future, documented): materializing back into a `Vec` still costs `min(pack, cap)` host RAM at
/// the end — true end-to-end streaming would need a `WireOutput`/`SandboxResult` streaming-body API
/// change. For CT-006c, disk-staging the drain (so RAM is bounded DURING the run) + a generous cap is
/// sufficient: real-size clones come through whole, and an over-cap response fails loud rather than
/// returning a truncated pack.
fn drain_to_temp_file<R: Read>(mut r: R, cap: usize) -> (Vec<u8>, bool) {
    use std::os::unix::fs::OpenOptionsExt;
    let path = std::env::temp_dir().join(format!(
        "myelin-gitwire-stdout-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = std::fs::remove_file(&path);
    // CT-007 slice 5b.2 (Sol's review): this path now also stages the checkout-preparation transport's
    // prefetched pack (a private source tree), not just CI/agent build logs -- `create_new` (never
    // follows a pre-existing symlink at `path`, unlike the previous plain `File::create`) + an explicit
    // `0600` (never a `File::create`-default, umask-dependent, commonly world-readable `0644`).
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
    {
        Ok(f) => f,
        // If we cannot stage to disk, fall back to the in-RAM capped drain under the SAME generous cap
        // (still bounded, still reports truncation) rather than losing the deadlock-free pipe drain.
        Err(_) => return drain_capped(&mut r, cap),
    };
    // Unlink immediately, through the retained fd (Sol's review) -- from here on this file is
    // reachable ONLY via `file`, matching `tempfile_for_checkout_pack`'s own anonymous-by-path
    // discipline, rather than staying path-visible for the whole capture. A failed unlink means it
    // is NOT actually anonymous -- fall back to the in-RAM capped drain (same bound, same
    // truncation reporting) rather than silently carrying wire content through a still
    // path-reachable file.
    if std::fs::remove_file(&path).is_err() {
        return drain_capped(&mut r, cap);
    }
    let mut written: usize = 0;
    let mut truncated = false;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => break, // EOF: the guest pipe closed (child exited / whole-container-killed).
            Ok(n) => {
                if written < cap {
                    let take = (cap - written).min(n);
                    if file.write_all(&chunk[..take]).is_err() {
                        // A disk write error ⇒ stop staging but KEEP draining to EOF (no pipe-fill
                        // deadlock); treat as truncated so the wire seam refuses rather than serving a
                        // short pack.
                        truncated = true;
                        written = cap;
                    } else {
                        written += take;
                        if take < n {
                            truncated = true; // overflowed the cap this read
                        }
                    }
                } else {
                    truncated = true; // already at the cap: drain + discard the remainder
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                // Preserve the diagnostic prefix, but force the wire seam to reject it as incomplete.
                truncated = true;
                break;
            }
        }
    }
    let _ = file.flush();
    // Read the staged bytes back through the SAME fd (Sol's review: never reopen by path for a
    // second, independent access to what may be sensitive wire content) by seeking to the start. A
    // read-back failure is treated as truncated (fail-closed: the wire seam refuses rather than
    // serving a short/empty pack).
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
    // Already unlinked above -- `path` names nothing on disk; `file` simply drops.
    (head, truncated)
}

/// Fallback usage for a [`RunFailure`] in the `Executed` phase (CT-007 vertical-slice step 2b's
/// gvisor.rs-integration planning, Sol's review): the runtime had already been released to exec
/// by the time the failure occurred, so it must never be charged zero (that would let a spawn
/// that ran real, if briefly, execute for free — a host-DoS surface via jobs engineered to fail
/// exactly this way). Deliberately similar to, but NOT reusing, [`build_result`]'s own formula:
/// this rounds elapsed time UP TO AT LEAST ONE SECOND (a `.max(1)` floor `build_result`'s own,
/// separately-tested success-path formula does not have and must not silently gain) — the
/// distinct floor Sol's review specified for a conservative FAILURE estimate, not a change to
/// already-tested successful-completion accounting.
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

/// Build the [`SandboxResult`] from the runtime outcome. The exit code is the `runsc` child's REAL
/// exit status (gVisor returns the container process's exit directly — no forge surface); a timed-out
/// (killed) container has no trustworthy exit (`None`, never fabricated as 0). stdout/stderr are the
/// container's REAL piped streams, HEAD-bounded to [`SANDBOX_CAPTURE_BOUND`] (256 KiB) EACH — the same
/// bound the Firecracker backend uses (shared `lib.rs` const). Usage is the REAL measured figure: host
/// CPU-seconds of the `runsc` process (or a wall-clock ceiling fallback so a real run never
/// under-meters to 0) + mem-byte-seconds from the job's mem ceiling × wall-seconds.
pub(super) fn build_result(spec: &JobSpec, o: &RunscOutcome, redaction: &RedactionPlan) -> SandboxResult {
    // BOUNDARY REDACTION (CT-004f sub-step 1): mask the job's CI-managed secret needles in the captured
    // streams HERE — the last step before the bytes populate `SandboxResult` and cross back toward the
    // durable log pipeline — so no injected secret is sealed into the content-addressed log store. The
    // plan is empty for non-secret jobs and populated from the same resolved bindings as OCI env for
    // secret-bearing jobs. It is a REQUIRED argument so no capture path can forward un-redacted bytes.
    // Redaction runs on the already-per-stream-bounded bytes.
    //
    // The drain threads ALREADY applied the correct per-stream bound ([`run_and_capture`]): stdout is
    // bounded by its [`StdoutMode`] (256 KiB head for CI/agent; the generous git-wire cap, disk-staged),
    // stderr by [`SANDBOX_CAPTURE_BOUND`]. Re-truncating stdout to 256 KiB HERE would corrupt a real-size
    // wire packfile (CT-006c FU-1 — the original silent-truncation defect), so the already-bounded bytes
    // pass through unchanged. stderr is belt-and-braces clamped (it is already ≤ the bound).
    let stdout = redaction.redact(&o.stdout);
    let mut stderr = redaction.redact(&o.stderr);
    if stderr.len() > SANDBOX_CAPTURE_BOUND {
        stderr.truncate(SANDBOX_CAPTURE_BOUND);
    }
    // A timed-out container was killed mid-flight ⇒ no trustworthy exit code (do NOT fabricate one).
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

/// The post-run container teardown handle (the container has already exited/been-killed + been
/// deleted by the run path). [`kill`](RunscChild::kill) is an idempotent best-effort `runsc delete
/// -force` (no-op if already gone). [`wait`](RunscChild::wait) is a no-op for trait parity with the
/// Firecracker `VmmChild` — the REAL exit was already captured from the `runsc` child's exit status.
pub(super) struct SpawnedRunsc {
    pub(super) bin: &'static Path,
    pub(super) container_id: String,
    /// The SAME mode this container was launched with (CT-007 slice 2) — `kill`'s `delete`
    /// invocation MUST use identical global flags to the original `run`, or `runsc` may fail to
    /// locate/manage a container whose namespace/cgroup posture it was never told to expect.
    pub(super) mode: RunscInvocationMode,
}
impl RunscChild for SpawnedRunsc {
    fn kill(&mut self) -> Result<(), String> {
        delete_container(self.bin, &self.container_id, self.mode);
        Ok(())
    }
    fn wait(&mut self) -> Result<i32, String> {
        // The real exit was already captured from the `runsc` child's exit status (no real wait left).
        Ok(0)
    }
}

//! Durable launch authorization at the child-spawn boundary.
//!
//! Production CI must not execute guest code until its exact durable launch CAS commits. It also
//! must not commit that CAS before there is a child process to own the launch. This module joins
//! those facts mechanically:
//!
//! 1. spawn a host-side guard in a fresh process group, blocked on stdin;
//! 2. commit the [`LaunchPermit`](crate::LaunchPermit) while the guard cannot exec the runtime;
//! 3. release one byte only after commit succeeds, at which point the guard execs the real runtime.
//!
//! A second close-on-exec pipe is held by the runner. A native watchdog child waits for either EOF
//! or a kernel `CLOCK_BOOTTIME` deadline and SIGKILLs the complete process group (plus gVisor's
//! complete cgroup). The watcher remains runnable when the runner process is stopped, and
//! `CLOCK_BOOTTIME` accounts elapsed host-suspend time when the host resumes. Thus a crash before
//! commit rolls back the CAS and kills the blocked guard; a crash or stopped runner after commit
//! cannot let the runtime outlive its durable execution lease.

use crate::LaunchPermit;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const GATE_SCRIPT: &str = r#"
IFS= read -r myelin_launch_gate || exit 125
exec "$@"
"#;

/// What happens to the launch-gate pipe's write end, held by the parent, once the release byte has
/// been written and the guard's `exec "$@"` is free to run. The SAME pipe that carries the
/// gate-release line is also the exec'd runtime's inherited stdin — for a caller with nothing
/// further to say (every CI/agent job), the pipe must be closed immediately so the guest sees EOF
/// on an empty stdin, exactly as intended. A caller that needs to feed real bytes to the guest
/// AFTER release (the git-wire request body) must ask to have the SAME live pipe handed back
/// instead, via [`SandboxCommand::return_stdin_to_caller_after_gate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PostGateStdin {
    /// Close the pipe the instant the release byte lands (the default/legacy behavior).
    #[default]
    Close,
    /// Hand the still-open pipe back to the caller via [`SandboxChild::stdin`], but only once
    /// `ownership.release()` has actually succeeded -- see `spawn()`'s own ordering comment for why
    /// restoring it any earlier would risk a hung `wait()` on a release failure.
    ReturnToCaller,
}

/// Command whose fenced form cannot exec its runtime until the durable launch CAS commits. `None`
/// is reserved for paths such as git-wire that have no durable scheduler claim.
pub(crate) struct SandboxCommand {
    command: Command,
    permit: Option<LaunchPermit>,
    liveness_read: Option<OwnedFd>,
    liveness_write: Option<OwnedFd>,
    ready_read: Option<OwnedFd>,
    ready_write: Option<OwnedFd>,
    cgroup_kill: Option<File>,
    watchdog_timeout: Option<Duration>,
    fenced: bool,
    post_gate_stdin: PostGateStdin,
}

impl SandboxCommand {
    pub(crate) fn new(
        program: impl AsRef<OsStr>,
        permit: Option<LaunchPermit>,
        watchdog_timeout: Option<Duration>,
    ) -> io::Result<Self> {
        let Some(permit) = permit else {
            if watchdog_timeout.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "an unfenced sandbox command cannot carry a launch watchdog deadline",
                ));
            }
            return Ok(Self {
                command: Command::new(program),
                permit: None,
                liveness_read: None,
                liveness_write: None,
                ready_read: None,
                ready_write: None,
                cgroup_kill: None,
                watchdog_timeout: None,
                fenced: false,
                post_gate_stdin: PostGateStdin::Close,
            });
        };
        let watchdog_timeout = watchdog_timeout
            .filter(|timeout| !timeout.is_zero())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a fenced sandbox command requires a positive watchdog deadline",
                )
            })?;

        let (liveness_read, liveness_write) = cloexec_pipe()?;
        let (ready_read, ready_write) = cloexec_pipe()?;
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(GATE_SCRIPT)
            .arg("myelin-launch-gate")
            .arg(program)
            .stdin(Stdio::piped());

        Ok(Self {
            command,
            permit: Some(permit),
            liveness_read: Some(liveness_read),
            liveness_write: Some(liveness_write),
            ready_read: Some(ready_read),
            ready_write: Some(ready_write),
            cgroup_kill: None,
            watchdog_timeout: Some(watchdog_timeout),
            fenced: true,
            post_gate_stdin: PostGateStdin::Close,
        })
    }

    pub(crate) fn command_mut(&mut self) -> &mut Command {
        &mut self.command
    }

    pub(crate) fn is_fenced(&self) -> bool {
        self.fenced
    }

    /// Ask that the launch-gate pipe's write end be handed back (via [`SandboxChild::stdin`])
    /// instead of closed once the release byte lands and durable ownership is released -- for a
    /// caller (git-wire) that needs to feed real bytes into the guest's stdin after the gate opens.
    /// Panics on an unfenced command: there is no gate pipe to retain (see `PostGateStdin`'s doc for
    /// the "why can only a fenced command have anything to retain" reasoning).
    pub(crate) fn return_stdin_to_caller_after_gate(&mut self) {
        assert!(
            self.fenced,
            "only a fenced sandbox command has a launch-gate pipe to retain"
        );
        self.post_gate_stdin = PostGateStdin::ReturnToCaller;
    }

    /// Give the liveness watchdog a kernel-owned whole-cgroup kill switch. gVisor may create
    /// sentry/gofer processes in their own process groups, so process-group cleanup alone is not
    /// sufficient for that backend. Every descendant remains in the host-owned memory cgroup.
    pub(crate) fn kill_cgroup_on_liveness_loss(&mut self, kill_file: File) {
        self.cgroup_kill = Some(kill_file);
    }

    /// Spawn the blocked guard, durably commit the launch CAS, then release the runtime. Any error
    /// before release kills and reaps the complete still-trusted process group.
    ///
    /// The error carries a [`SpawnPhase`] disposition (CT-007 vertical-slice step 2b's
    /// gvisor.rs-integration planning found a real, pre-existing gap this exists to close): a
    /// caller that only sees a flat `String` cannot tell "nothing durable happened" (safe to
    /// release the caller's cost reservation at zero) apart from "the durable launch CAS
    /// committed, but the runtime never got to exec" (the reservation's cost is real even though
    /// no workload ran — it must be settled at zero, not released) apart from "the runtime may
    /// ALREADY be executing" (the release-byte write below succeeds and the guard's `exec "$@"`
    /// runs immediately after — a failure AFTER that point must be accounted as a real, if
    /// botched, execution, never charged for free).
    pub(crate) fn spawn(mut self) -> Result<SandboxChild, SpawnFailure> {
        let watchdog_timer = if self.fenced {
            Some(
                boottime_timer(
                    self.watchdog_timeout
                        .expect("fenced sandbox command owns a watchdog deadline"),
                )
                .map_err(|error| {
                    SpawnFailure::uncommitted(
                        format!("arm sandbox launch watchdog deadline: {error}"),
                        DirectChildRetirement::NoChildReturned,
                    )
                })?,
            )
        } else {
            None
        };
        if self.fenced {
            let liveness_fd = self
                .liveness_read
                .as_ref()
                .expect("fenced sandbox command owns a liveness pipe")
                .as_raw_fd();
            let liveness_write_fd = self
                .liveness_write
                .as_ref()
                .expect("fenced sandbox command owns a liveness writer")
                .as_raw_fd();
            let ready_fd = self
                .ready_write
                .as_ref()
                .expect("fenced sandbox command owns a readiness writer")
                .as_raw_fd();
            let timer_fd = watchdog_timer
                .as_ref()
                .expect("fenced sandbox command owns a watchdog timer")
                .as_raw_fd();
            let cgroup_kill_fd = self.cgroup_kill.as_ref().map_or(-1, AsRawFd::as_raw_fd);
            // SAFETY: the pre-exec parent branch uses only async-signal-safe libc calls. The
            // watchdog child never returns into Rust: it closes every inherited descriptor except
            // its exact pipes/timer/cgroup switch, signals readiness, polls, kills, and `_exit`s.
            unsafe {
                self.command.pre_exec(move || {
                    if libc::setpgid(0, 0) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    let process_group = libc::getpid();
                    let watchdog = libc::fork();
                    if watchdog == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    if watchdog == 0 {
                        // The watchdog must survive a parent-driven kill of the runtime group long
                        // enough to close the gVisor cgroup. It therefore owns a separate process
                        // group while retaining the original runtime PGID as its kill target.
                        if libc::setpgid(0, 0) == -1 {
                            libc::_exit(126);
                        }
                        native_watchdog(
                            liveness_fd,
                            liveness_write_fd,
                            ready_fd,
                            timer_fd,
                            cgroup_kill_fd,
                            process_group,
                        );
                    }
                    Ok(())
                });
            }
        }
        // Unfenced commands (no launch permit — e.g. the host preflight self-test) have no
        // commit/gate boundary at all — the process spawn itself IS the moment execution starts.
        // Fenced commands capture this later, immediately before the gate write (the true
        // release-to-exec moment).
        let mut executed_at = (!self.fenced).then(Instant::now);
        let mut child = self.command.spawn().map_err(|error| {
            SpawnFailure::uncommitted(
                format!("spawn sandbox process: {error}"),
                DirectChildRetirement::NoChildReturned,
            )
        })?;
        drop(self.liveness_read.take());
        drop(self.ready_write.take());
        drop(self.cgroup_kill.take());

        if self.fenced {
            let group_id = child.id() as i32;
            let mut gate = match child.stdin.take() {
                Some(gate) => gate,
                None => {
                    kill_process_group(group_id);
                    let child_retirement = DirectChildRetirement::from_wait_result(child.wait());
                    return Err(SpawnFailure::uncommitted(
                        "launch guard gate pipe unavailable".to_string(),
                        child_retirement,
                    ));
                }
            };
            let ready_result = wait_until_ready(
                self.ready_read
                    .as_mut()
                    .expect("fenced sandbox command owns a readiness pipe"),
            );
            drop(self.ready_read.take());
            if let Err(error) = ready_result {
                // Explicit EOF backstop, same reasoning as every other failure branch below: even
                // though the guard hasn't been released yet (so it's still blocked on its own
                // `read -r`), closing our write end before the kill/wait gives the guard an
                // independent way to unblock and exit if the process-group signal is ever missed.
                drop(gate);
                kill_process_group(group_id);
                let child_retirement = DirectChildRetirement::from_wait_result(child.wait());
                return Err(SpawnFailure::uncommitted(
                    format!("launch guard failed to arm liveness watchdog: {error}"),
                    child_retirement,
                ));
            }
            let permit = self
                .permit
                .take()
                .expect("fenced sandbox command owns one launch permit");
            let ownership = match permit.commit() {
                Ok(ownership) => ownership,
                Err(error) => {
                    drop(gate);
                    kill_process_group(group_id);
                    let child_retirement = DirectChildRetirement::from_wait_result(child.wait());
                    // The CAS returned an error, but that does NOT prove nothing committed: the
                    // durable store may have committed and lost the acknowledgement (e.g. a
                    // Postgres commit whose result never reached the caller). Calling this
                    // Uncommitted would let a caller release a reservation the store may still
                    // consider owned. Neither release nor settle — surface the ambiguity so the
                    // caller defers to durable reconciliation instead of guessing.
                    return Err(SpawnFailure::commit_outcome_unknown(
                        format!(
                            "durable launch commit returned an error before sandbox exec: {error}"
                        ),
                        child_retirement,
                    ));
                }
            };
            let ownership = match ownership.validate() {
                Ok(ownership) => ownership,
                Err(error) => {
                    drop(gate);
                    kill_process_group(group_id);
                    let child_retirement = DirectChildRetirement::from_wait_result(child.wait());
                    // The CAS DID commit (durably); the runtime is being killed before exec — the
                    // reservation's cost is real even though no workload ran.
                    return Err(SpawnFailure::committed_but_not_executed(
                        format!("durable launch ownership was lost before sandbox exec: {error}"),
                        child_retirement,
                    ));
                }
            };
            // Captured IMMEDIATELY BEFORE the write below, not after: once the byte is written the
            // guard's `exec "$@"` could start at any moment, so this must be the TRUE release
            // moment a later Executed-phase failure, and the eventual successful `SandboxChild`,
            // report elapsed time from — never a later point after the (possibly slow) write
            // syscall or this whole call has returned.
            let release_at = Instant::now();
            if let Err(error) = gate.write_all(b"launch\n") {
                drop(gate);
                kill_process_group(group_id);
                let child_retirement = DirectChildRetirement::from_wait_result(child.wait());
                // Committed, but the guard never even read the release byte — still no exec. The
                // write failed, so `release_at` is discarded (never assigned to `executed_at`) —
                // this phase carries no execution timestamp.
                return Err(SpawnFailure::committed_but_not_executed(
                    format!("release sandbox after durable launch commit: {error}"),
                    child_retirement,
                ));
            }
            executed_at = Some(release_at);
            // `gate` must not be restored into `child.stdin` until `ownership.release()` has
            // actually succeeded — restoring it any earlier would leave an open writer inside
            // `child` on the release-failure path below: if the best-effort `kill_process_group`
            // failed to actually stop the runtime, `child.wait()` could then hang behind a guest
            // still waiting for EOF on a pipe this function itself kept open.
            match self.post_gate_stdin {
                PostGateStdin::Close => {
                    drop(gate);
                    if let Err(error) = ownership.release() {
                        // The gate byte WAS written above — the guard's `exec "$@"` may already be
                        // running by the time this specific release call failed. Treat as a real
                        // (if botched) execution: the caller must account conservative fallback
                        // usage, never charge zero for it.
                        kill_process_group(group_id);
                        let child_retirement =
                            DirectChildRetirement::from_wait_result(child.wait());
                        return Err(SpawnFailure::executed(
                            format!(
                                "release durable launch ownership after sandbox exec handoff: {error}"
                            ),
                            executed_at.expect("executed_at was just set above"),
                            child_retirement,
                        ));
                    }
                }
                PostGateStdin::ReturnToCaller => match ownership.release() {
                    Ok(()) => child.stdin = Some(gate),
                    Err(error) => {
                        // Independent EOF backstop before the kill/wait below, same reasoning as
                        // every earlier branch: don't rely solely on the process-group signal.
                        drop(gate);
                        kill_process_group(group_id);
                        let child_retirement =
                            DirectChildRetirement::from_wait_result(child.wait());
                        return Err(SpawnFailure::executed(
                            format!(
                                "release durable launch ownership after sandbox exec handoff: {error}"
                            ),
                            executed_at.expect("executed_at was just set above"),
                            child_retirement,
                        ));
                    }
                },
            }
        }

        let process_group = self.fenced.then_some(child.id() as i32);
        Ok(SandboxChild {
            child,
            liveness_write: self.liveness_write,
            watchdog_timer,
            process_group,
            executed_at: executed_at
                .expect("executed_at is always set (pre-spawn if unfenced, pre-gate-write if fenced) by the time spawn succeeds"),
        })
    }
}

/// Whether the sandbox's DIRECT child process (the `runsc` runtime `SandboxCommand` spawned — not
/// any container it may itself have started) was confirmed reaped by the time a
/// [`SandboxCommand::spawn`] failure, or a later [`SandboxChild::kill_and_wait`] call, returned.
/// CT-007 slice 3, piece 7b (Sol's design review): `wait()`'s own `Result` was previously discarded
/// everywhere it was called on a failure path — a `wait()` failure (e.g. ECHILD from a reaper race,
/// or an I/O error) does NOT mean the process is gone, only that ITS FATE IS UNKNOWN. Callers that
/// need to mint teardown evidence must be able to tell "confirmed gone" apart from "no idea".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectChildRetirement {
    /// The caller never acquired a child handle — either `Command::spawn` itself never ran (e.g. a
    /// pre-spawn watchdog-arming failure), or it was attempted and returned `Err`. NOT a claim that
    /// no process was ever created: a failed pre-exec/exec attempt can still have forked a process
    /// this handle never learned the pid of. Cgroup quiescence (identified independently by its own
    /// (device, inode), never by this handle) is the backstop that catches such a descendant.
    NoChildReturned,
    /// `wait()` returned a real exit status — the process is confirmed gone.
    Reaped,
    /// `wait()` itself failed — the process's fate is NOT confirmed; it may still be alive.
    Unconfirmed(String),
}

impl DirectChildRetirement {
    fn from_wait_result(result: io::Result<ExitStatus>) -> Self {
        match result {
            Ok(_) => DirectChildRetirement::Reaped,
            Err(error) => DirectChildRetirement::Unconfirmed(error.to_string()),
        }
    }
}

/// Which of four phases a [`SandboxCommand::spawn`] failure occurred in — see that method's own
/// doc for the accounting rule each phase implies for the caller's cost reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpawnPhase {
    /// No durable launch CAS committed. Safe to release the caller's reservation at zero cost.
    Uncommitted,
    /// The durable launch CAS returned an error, but whether it actually committed durably is
    /// UNKNOWN (e.g. the store committed but the acknowledgement was lost). Neither release nor
    /// settle — the caller must defer to durable reconciliation rather than guess either way.
    CommitOutcomeUnknown,
    /// The durable launch CAS committed, but the runtime never got to exec (or was killed before
    /// it could). The reservation's cost is real (the commit itself is a durable, accounted
    /// fact) even though no workload ran — settle at zero, never release-as-unused.
    CommittedButNotExecuted,
    /// The runtime may already be executing (the release byte was written) by the time this
    /// specific failure occurred. The caller must settle CONSERVATIVE fallback usage — never
    /// zero, which would let a spawn that ran real (if briefly) execute for free.
    Executed,
}

/// A phase-tagged [`SandboxCommand::spawn`] failure. `executed_at` is populated ONLY for the
/// `Executed` phase — the `Instant` captured immediately before the release byte was written (the
/// earliest moment the guard's `exec "$@"` could have started) — so a caller with no OTHER
/// elapsed-time reference (`spawn()` itself failed, so the caller never got a `child`/start-time
/// of its own) can still compute a conservative fallback usage from `executed_at.elapsed()`.
///
/// `child_retirement` (CT-007 slice 3, piece 7b) carries whatever this method itself already knows
/// about the DIRECT child's fate — [`DirectChildRetirement::NoChildReturned`] for every failure before
/// `Command::spawn` succeeded, or the real `wait()` outcome (no longer discarded) for every failure
/// after a child existed. A caller building teardown evidence needs this instead of assuming
/// "spawn() failed" implies "nothing to reap".
#[derive(Debug)]
pub(crate) struct SpawnFailure {
    phase: SpawnPhase,
    message: String,
    executed_at: Option<Instant>,
    child_retirement: DirectChildRetirement,
}

impl SpawnFailure {
    fn uncommitted(message: String, child_retirement: DirectChildRetirement) -> Self {
        Self {
            phase: SpawnPhase::Uncommitted,
            message,
            executed_at: None,
            child_retirement,
        }
    }

    fn commit_outcome_unknown(message: String, child_retirement: DirectChildRetirement) -> Self {
        Self {
            phase: SpawnPhase::CommitOutcomeUnknown,
            message,
            executed_at: None,
            child_retirement,
        }
    }

    fn committed_but_not_executed(
        message: String,
        child_retirement: DirectChildRetirement,
    ) -> Self {
        Self {
            phase: SpawnPhase::CommittedButNotExecuted,
            message,
            executed_at: None,
            child_retirement,
        }
    }

    fn executed(
        message: String,
        executed_at: Instant,
        child_retirement: DirectChildRetirement,
    ) -> Self {
        Self {
            phase: SpawnPhase::Executed,
            message,
            executed_at: Some(executed_at),
            child_retirement,
        }
    }

    pub(crate) fn phase(&self) -> SpawnPhase {
        self.phase
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    /// The `Instant` captured immediately before the release byte was written — `Some` only for
    /// `phase() == SpawnPhase::Executed`. Use `.elapsed()` on this to compute conservative fallback
    /// usage.
    pub(crate) fn executed_at(&self) -> Option<Instant> {
        self.executed_at
    }

    /// Whether the DIRECT child (if one was ever spawned) is confirmed reaped by the time this
    /// failure occurred. See [`DirectChildRetirement`].
    pub(crate) fn child_retirement(&self) -> &DirectChildRetirement {
        &self.child_retirement
    }
}

impl std::fmt::Display for SpawnFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SpawnFailure {}

/// Child returned by [`SandboxCommand`]. Closing `liveness_write` after the process leader exits
/// releases the watchdog, which removes surviving descendants and then kills itself.
#[derive(Debug)]
pub(crate) struct SandboxChild {
    child: Child,
    liveness_write: Option<OwnedFd>,
    watchdog_timer: Option<OwnedFd>,
    process_group: Option<i32>,
    executed_at: Instant,
}

impl SandboxChild {
    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }

    /// The TRUE moment execution began: for a fenced launch, immediately before the gate-release
    /// byte was written (the earliest point the guard's `exec "$@"` could start); for an unfenced
    /// launch, immediately before `Command::spawn`. Use this (never a later `Instant::now()`
    /// captured after `spawn()` already returned `Ok`) as the base for every elapsed-time
    /// computation — gate-release/pipe-setup time is real execution time the caller must not
    /// silently exclude from usage accounting.
    pub(crate) fn executed_at(&self) -> Instant {
        self.executed_at
    }

    pub(crate) fn stdin(&mut self) -> &mut Option<ChildStdin> {
        &mut self.child.stdin
    }

    pub(crate) fn stdout(&mut self) -> &mut Option<ChildStdout> {
        &mut self.child.stdout
    }

    pub(crate) fn stderr(&mut self) -> &mut Option<ChildStderr> {
        &mut self.child.stderr
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.finish_liveness();
        }
        Ok(status)
    }

    pub(crate) fn watchdog_deadline_expired(&self) -> bool {
        let Some(timer) = self.watchdog_timer.as_ref() else {
            return false;
        };
        let mut poll_fd = libc::pollfd {
            fd: timer.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `poll_fd` is one valid descriptor and the zero timeout never blocks.
        unsafe { libc::poll(&mut poll_fd, 1, 0) == 1 && poll_fd.revents & libc::POLLIN != 0 }
    }

    /// Kill and reap the direct child, returning whether the reap is actually confirmed (CT-007
    /// slice 3, piece 7b) — `wait()`'s result was previously discarded here, so a caller could not
    /// tell "confirmed gone" apart from "no idea" when building teardown evidence.
    pub(crate) fn kill_and_wait(&mut self) -> DirectChildRetirement {
        if let Some(group_id) = self.process_group {
            kill_process_group(group_id);
        } else {
            let _ = self.child.kill();
        }
        let child_retirement = DirectChildRetirement::from_wait_result(self.child.wait());
        self.finish_liveness();
        child_retirement
    }

    pub(crate) fn finish_liveness(&mut self) {
        drop(self.liveness_write.take());
    }
}

impl Drop for SandboxChild {
    fn drop(&mut self) {
        // A capture error must never orphan a sandbox. Normal completion has already reaped the
        // leader; kill(ESRCH) and a second wait are harmless then.
        if let Some(group_id) = self.process_group {
            kill_process_group(group_id);
        } else {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        self.finish_liveness();
    }
}

fn kill_process_group(group_id: i32) {
    if group_id > 0 {
        // SAFETY: a negative pid addresses exactly the fresh process group created in pre_exec.
        unsafe {
            libc::kill(-group_id, libc::SIGKILL);
        }
    }
}

fn boottime_timer(timeout: Duration) -> io::Result<OwnedFd> {
    // SAFETY: timerfd_create returns one newly-owned descriptor on success.
    let fd = unsafe { libc::timerfd_create(libc::CLOCK_BOOTTIME, libc::TFD_CLOEXEC) };
    if fd == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful timerfd_create transferred ownership of `fd`.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let seconds = libc::time_t::try_from(timeout.as_secs()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "sandbox watchdog deadline exceeds time_t",
        )
    })?;
    let value = libc::itimerspec {
        it_interval: libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        },
        it_value: libc::timespec {
            tv_sec: seconds,
            tv_nsec: libc::c_long::from(timeout.subsec_nanos()),
        },
    };
    // SAFETY: `owned` is a timerfd and `value` is fully initialized.
    if unsafe { libc::timerfd_settime(owned.as_raw_fd(), 0, &value, std::ptr::null_mut()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(owned)
}

/// The watchdog runs after `fork` and never returns into Rust. Keep this path allocation-free and
/// limited to async-signal-safe Linux syscalls.
unsafe fn native_watchdog(
    liveness_fd: RawFd,
    liveness_write_fd: RawFd,
    ready_fd: RawFd,
    timer_fd: RawFd,
    cgroup_kill_fd: RawFd,
    process_group: libc::pid_t,
) -> ! {
    let keep = [liveness_fd, ready_fd, timer_fd, cgroup_kill_fd];
    unsafe {
        close_unneeded_fds(&keep);
        libc::close(liveness_write_fd);
        libc::close(libc::STDIN_FILENO);
        libc::close(libc::STDOUT_FILENO);
        libc::close(libc::STDERR_FILENO);
    }

    let ready = b"R";
    if unsafe { libc::write(ready_fd, ready.as_ptr().cast(), ready.len()) } != 1 {
        unsafe { kill_from_watchdog(cgroup_kill_fd, process_group) };
    }
    unsafe {
        libc::close(ready_fd);
    }

    let mut watched = [
        libc::pollfd {
            fd: liveness_fd,
            events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
            revents: 0,
        },
        libc::pollfd {
            fd: timer_fd,
            events: libc::POLLIN | libc::POLLERR,
            revents: 0,
        },
    ];
    loop {
        let result = unsafe { libc::poll(watched.as_mut_ptr(), watched.len() as libc::nfds_t, -1) };
        if result > 0 {
            unsafe { kill_from_watchdog(cgroup_kill_fd, process_group) };
        }
        if result == -1 {
            let errno = unsafe { *libc::__errno_location() };
            if errno == libc::EINTR {
                continue;
            }
        }
        unsafe { kill_from_watchdog(cgroup_kill_fd, process_group) };
    }
}

unsafe fn kill_from_watchdog(cgroup_kill_fd: RawFd, process_group: libc::pid_t) -> ! {
    if cgroup_kill_fd >= 0 {
        let kill = b"1";
        unsafe {
            libc::write(cgroup_kill_fd, kill.as_ptr().cast(), kill.len());
        }
    }
    if process_group > 0 {
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
    }
    unsafe {
        libc::_exit(0);
    }
}

unsafe fn close_unneeded_fds(keep: &[RawFd; 4]) {
    // The watchdog forks before the wrapper exec, so CLOEXEC has not run yet. Close everything
    // except the four exact kernel capabilities; otherwise it could keep unrelated pooled sockets
    // or pipe writers alive. A fixed insertion sort avoids allocation in the post-fork child.
    let mut ordered = *keep;
    for index in 1..ordered.len() {
        let mut cursor = index;
        while cursor > 0 && ordered[cursor - 1] > ordered[cursor] {
            ordered.swap(cursor - 1, cursor);
            cursor -= 1;
        }
    }
    let mut next = 3u32;
    let mut close_range_available = true;
    for fd in ordered {
        if fd < 3 {
            continue;
        }
        let fd = fd as u32;
        if next < fd && close_range_available {
            let result =
                unsafe { libc::syscall(libc::SYS_close_range, next, fd - 1, 0u32) as libc::c_int };
            if result == -1 {
                close_range_available = false;
            }
        }
        next = fd.saturating_add(1);
    }
    if close_range_available {
        let result =
            unsafe { libc::syscall(libc::SYS_close_range, next, u32::MAX, 0u32) as libc::c_int };
        if result != -1 {
            return;
        }
    }

    let mut limit = libc::rlimit {
        rlim_cur: 65_536,
        rlim_max: 65_536,
    };
    unsafe {
        libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit);
    }
    let ceiling = limit.rlim_cur.min(1_048_576) as RawFd;
    for fd in 3..ceiling {
        if !keep.contains(&fd) {
            unsafe {
                libc::close(fd);
            }
        }
    }
}

fn cloexec_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    // SAFETY: `fds` points to two valid integers; successful return transfers both descriptors.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pipe2 returned two newly-owned descriptors.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn wait_until_ready(pipe: &mut OwnedFd) -> io::Result<()> {
    let mut poll_fd = libc::pollfd {
        fd: pipe.as_raw_fd(),
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    };
    loop {
        // SAFETY: `poll_fd` is one initialized pollfd and remains valid for this call.
        let result = unsafe { libc::poll(&mut poll_fd, 1, 5_000) };
        if result > 0 {
            let mut ready = [0u8; 1];
            // SAFETY: `ready` is a valid one-byte output buffer and the polled fd remains owned.
            let read = unsafe {
                libc::read(
                    pipe.as_raw_fd(),
                    ready.as_mut_ptr().cast::<libc::c_void>(),
                    ready.len(),
                )
            };
            if read == -1 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            return if read == 1 && ready == [b'R'] {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "launch guard returned an invalid readiness byte",
                ))
            };
        }
        if result == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "launch guard did not arm within 5 seconds",
            ));
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Test-support entrypoint run in a disposable helper process. The caller SIGKILLs that process
/// after `armed` appears; the real liveness pipe must then kill the delayed runtime before it can
/// create `escaped`.
#[cfg(any(test, feature = "test-support"))]
pub fn launch_gate_parent_death_probe(
    armed: &std::path::Path,
    escaped: &std::path::Path,
) -> Result<(), String> {
    let permit = LaunchPermit::retained(|| Ok(crate::LaunchOwnership::immediate()));
    let mut command =
        SandboxCommand::new("/bin/sh", Some(permit), Some(Duration::from_millis(500)))
            .map_err(|error| format!("prepare process-death probe: {error}"))?;
    command
        .command_mut()
        .arg("-c")
        .arg("sleep 1; printf escaped > \"$1\"; sleep 5")
        .arg("myelin-process-death-probe")
        .arg(escaped)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _owned_child = command.spawn().map_err(|error| error.to_string())?;
    std::fs::write(armed, b"armed")
        .map_err(|error| format!("publish process-death probe readiness: {error}"))?;
    loop {
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HookError;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    fn marker_path(label: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "myelin-launch-gate-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn marker_command(
        marker: &std::path::Path,
        script: &str,
        permit: LaunchPermit,
    ) -> SandboxCommand {
        let mut command =
            SandboxCommand::new("/bin/sh", Some(permit), Some(Duration::from_secs(2))).unwrap();
        command
            .command_mut()
            .arg("-c")
            .arg(script)
            .arg("myelin-test-runtime")
            .arg(marker)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    fn wait_for_exit(child: &mut SandboxChild) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if child.try_wait().unwrap().is_some() {
                return;
            }
            assert!(Instant::now() < deadline, "gated child did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn durable_commit_precedes_runtime_execution() {
        let marker = marker_path("ordered");
        let marker_at_commit = marker.clone();
        let permit = LaunchPermit::retained(move || {
            assert!(
                !marker_at_commit.exists(),
                "runtime executed before the durable launch commit"
            );
            Ok(crate::LaunchOwnership::immediate())
        });
        let command = marker_command(&marker, "printf ran > \"$1\"", permit);
        let mut child = command.spawn().expect("commit releases the gated runtime");
        wait_for_exit(&mut child);
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "ran");
        let _ = std::fs::remove_file(marker);
    }

    #[test]
    fn failed_commit_kills_guard_before_runtime_execution() {
        let marker = marker_path("commit-failed");
        let marker_at_commit = marker.clone();
        let permit = LaunchPermit::retained(move || {
            std::thread::sleep(Duration::from_millis(50));
            assert!(
                !marker_at_commit.exists(),
                "runtime escaped the gate while commit was in progress"
            );
            Err(HookError("injected commit failure".into()))
        });
        let command = marker_command(&marker, "printf escaped > \"$1\"", permit);
        let error = command
            .spawn()
            .expect_err("failed commit must refuse launch");
        assert!(error.message().contains("injected commit failure"));
        // The CAS returned an error — whether it actually committed durably is UNKNOWN (a Postgres
        // commit can return an error after the server committed and lost the acknowledgement).
        // This must NOT be classified `Uncommitted` (which would tell the caller it is safe to
        // release the reservation at zero — an outcome-unknown attempt must never be guessed at).
        assert_eq!(error.phase(), SpawnPhase::CommitOutcomeUnknown);
        assert!(error.executed_at().is_none());
        assert!(!marker.exists(), "failed commit executed the runtime");
    }

    #[test]
    fn lost_post_commit_ownership_kills_guard_before_runtime_execution() {
        let marker = marker_path("ownership-lost");
        let permit = LaunchPermit::retained(|| {
            Ok(crate::LaunchOwnership::retained(|| {
                Err(HookError("injected session ownership loss".into()))
            }))
        });
        let command = marker_command(&marker, "printf escaped > \"$1\"", permit);
        let error = command
            .spawn()
            .expect_err("lost post-commit ownership must refuse gate release");
        assert!(error.message().contains("injected session ownership loss"));
        // The CAS DID commit durably (ownership.validate() is what failed, AFTER commit()
        // succeeded) — the reservation's cost is real even though the guest never got to exec.
        assert_eq!(error.phase(), SpawnPhase::CommittedButNotExecuted);
        assert!(error.executed_at().is_none());
        assert!(
            !marker.exists(),
            "runtime executed after launch ownership was lost"
        );
    }

    #[test]
    fn gate_opens_while_validated_ownership_is_still_held() {
        let marker = marker_path("gate-before-unlock");
        let marker_at_release = marker.clone();
        let permit = LaunchPermit::retained(|| {
            Ok(crate::LaunchOwnership::retained(move || {
                Ok(crate::ValidatedLaunchOwnership::retained(move || {
                    let deadline = Instant::now() + Duration::from_secs(1);
                    while !marker_at_release.exists() {
                        assert!(
                            Instant::now() < deadline,
                            "runtime gate did not open while durable ownership remained held"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Ok(())
                }))
            }))
        });
        let command = marker_command(&marker, "printf gated > \"$1\"", permit);
        let mut child = command
            .spawn()
            .expect("gate opens before validated ownership release");
        wait_for_exit(&mut child);
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "gated");
        let _ = std::fs::remove_file(marker);
    }

    #[test]
    fn failed_post_gate_release_kills_runtime_before_delayed_effect() {
        let marker = marker_path("release-failed");
        let permit = LaunchPermit::retained(|| {
            Ok(crate::LaunchOwnership::retained(|| {
                Ok(crate::ValidatedLaunchOwnership::retained(|| {
                    Err(HookError("injected post-gate unlock failure".into()))
                }))
            }))
        });
        let command = marker_command(
            &marker,
            "sleep 0.25; printf escaped > \"$1\"; sleep 5",
            permit,
        );
        let error = command
            .spawn()
            .expect_err("post-gate ownership release failure must kill the runtime");
        assert!(error
            .message()
            .contains("injected post-gate unlock failure"));
        // The release byte WAS already written when `ownership.release()` failed — the guard's
        // `exec "$@"` may already be running. Must be Executed (never a phase that would let the
        // caller charge zero), and `executed_at` must be populated with a plausible timestamp.
        assert_eq!(error.phase(), SpawnPhase::Executed);
        assert!(
            error.executed_at().is_some(),
            "Executed phase always carries executed_at"
        );
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !marker.exists(),
            "runtime survived a failed post-gate ownership release"
        );
    }

    #[test]
    fn independent_boottime_deadline_kills_runtime_without_runner_progress() {
        let marker = marker_path("deadline");
        let permit = LaunchPermit::retained(|| Ok(crate::LaunchOwnership::immediate()));
        let mut command =
            SandboxCommand::new("/bin/sh", Some(permit), Some(Duration::from_millis(100))).unwrap();
        command
            .command_mut()
            .arg("-c")
            .arg("sleep 0.35; printf escaped > \"$1\"; sleep 5")
            .arg("myelin-deadline-runtime")
            .arg(&marker)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().expect("durable commit releases runtime");
        wait_for_exit(&mut child);
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            !marker.exists(),
            "runtime outlived the independent kernel deadline"
        );
    }

    #[test]
    fn runner_liveness_loss_kills_the_complete_runtime_group() {
        let marker = marker_path("runner-died");
        let permit = LaunchPermit::retained(|| Ok(crate::LaunchOwnership::immediate()));
        let command = marker_command(
            &marker,
            "sleep 0.25; printf escaped > \"$1\"; sleep 5",
            permit,
        );
        let mut child = command.spawn().expect("durable commit releases runtime");

        // EOF on this exact pipe is what kernel fd teardown delivers when the runner process dies.
        child.finish_liveness();
        wait_for_exit(&mut child);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !marker.exists(),
            "runtime survived loss of its owning runner process"
        );
    }

    // --- task #33 regression coverage: PostGateStdin (retain-vs-close the launch-gate pipe) ---

    #[test]
    fn default_fenced_mode_closes_stdin_and_delivers_immediate_eof() {
        let marker = marker_path("default-close-eof");
        let permit = LaunchPermit::retained(|| Ok(crate::LaunchOwnership::immediate()));
        // `cat` blocks until it observes EOF on stdin; if the guest's stdin were still open (the
        // pre-fix bug's mirror image — never closing it at all — is not what we're guarding here,
        // but proving termination proves EOF arrived promptly, not that the guest hung reading).
        let command = marker_command(&marker, "cat > \"$1\"", permit);
        let mut child = command.spawn().expect("durable commit releases runtime");
        assert!(
            child.stdin().is_none(),
            "default (Close) mode must not expose a stdin pipe to the caller"
        );
        wait_for_exit(&mut child);
        assert_eq!(
            std::fs::read(&marker).unwrap(),
            b"",
            "guest must have seen immediate EOF on an unretained fenced stdin"
        );
        let _ = std::fs::remove_file(marker);
    }

    #[test]
    fn retained_mode_returns_exactly_one_live_stdin_pipe() {
        let marker = marker_path("retained-live-pipe");
        let permit = LaunchPermit::retained(|| Ok(crate::LaunchOwnership::immediate()));
        let mut command = marker_command(&marker, "cat > \"$1\"", permit);
        command.return_stdin_to_caller_after_gate();
        let mut child = command.spawn().expect("durable commit releases runtime");
        let pipe = child
            .stdin()
            .take()
            .expect("retained mode must hand back exactly one live stdin pipe");
        assert!(
            child.stdin().is_none(),
            "the pipe must not still be duplicated inside the child after being taken once"
        );
        drop(pipe);
        wait_for_exit(&mut child);
        let _ = std::fs::remove_file(marker);
    }

    fn assert_retained_stdin_roundtrip(label: &str, payload: &[u8]) {
        let marker = marker_path(label);
        let permit = LaunchPermit::retained(|| Ok(crate::LaunchOwnership::immediate()));
        let mut command = marker_command(&marker, "cat > \"$1\"", permit);
        command.return_stdin_to_caller_after_gate();
        let mut child = command.spawn().expect("durable commit releases runtime");
        let mut pipe = child
            .stdin()
            .take()
            .expect("retained mode must hand back a live stdin pipe");
        pipe.write_all(payload)
            .expect("writing the request payload must succeed");
        drop(pipe); // closes the write end -> delivers EOF, exactly gvisor.rs's own pattern
        wait_for_exit(&mut child);
        assert_eq!(
            std::fs::read(&marker).unwrap(),
            payload,
            "payload must arrive byte-exact after the shell gate consumed only its own release line"
        );
        let _ = std::fs::remove_file(marker);
    }

    #[test]
    fn retained_stdin_payload_with_embedded_newlines_arrives_byte_exact() {
        assert_retained_stdin_roundtrip("retained-newlines", b"line one\nline two\n\nline four\n");
    }

    #[test]
    fn retained_stdin_payload_that_looks_like_another_release_line_arrives_byte_exact() {
        // Proves the shell gate's `read -r` consumed exactly ITS OWN release line and nothing of
        // this payload's own leading "launch\n" text is swallowed as a second release.
        assert_retained_stdin_roundtrip("retained-launch-prefix", b"launch\nrest of the body");
    }

    #[test]
    fn retained_stdin_payload_with_nul_bytes_arrives_byte_exact() {
        assert_retained_stdin_roundtrip("retained-nul", b"before\0after\0\0end");
    }

    #[test]
    fn retained_stdin_empty_payload_delivers_immediate_eof() {
        assert_retained_stdin_roundtrip("retained-empty", b"");
    }

    #[test]
    fn retained_mode_requested_but_lost_post_commit_ownership_never_exposes_stdin() {
        let marker = marker_path("retained-ownership-lost");
        let permit = LaunchPermit::retained(|| {
            Ok(crate::LaunchOwnership::retained(|| {
                Err(HookError("injected session ownership loss".into()))
            }))
        });
        let mut command = marker_command(&marker, "cat > \"$1\"", permit);
        command.return_stdin_to_caller_after_gate();
        let start = Instant::now();
        let error = command.spawn().expect_err(
            "lost post-commit ownership must refuse gate release even in retained mode",
        );
        // Same structural argument as the commit-failure test below: a failed `spawn()` never
        // returns a `SandboxChild`, so there is no way to reach a stdin handle here regardless of
        // the requested `PostGateStdin` policy.
        assert_eq!(error.phase(), SpawnPhase::CommittedButNotExecuted);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "a retained-mode validation failure must return promptly"
        );
        assert!(
            !marker.exists(),
            "runtime executed after launch ownership was lost in retained mode"
        );
    }

    #[test]
    fn retained_mode_requested_but_failed_commit_never_exposes_stdin() {
        let marker = marker_path("retained-commit-failed");
        let marker_at_commit = marker.clone();
        let permit = LaunchPermit::retained(move || {
            assert!(!marker_at_commit.exists(), "runtime executed before commit");
            Err(HookError("injected commit failure".into()))
        });
        let mut command = marker_command(&marker, "cat > \"$1\"", permit);
        command.return_stdin_to_caller_after_gate();
        let error = command
            .spawn()
            .expect_err("failed commit must refuse launch even in retained mode");
        // A failed `spawn()` never returns a `SandboxChild` at all, so there is structurally no way
        // to reach a stdin handle here — this asserts the retained-mode request didn't change the
        // pre-existing commit-failure phase/behavior in any way.
        assert_eq!(error.phase(), SpawnPhase::CommitOutcomeUnknown);
        assert!(!marker.exists(), "failed commit executed the runtime");
    }

    #[test]
    fn failed_post_gate_release_in_retained_mode_drops_stdin_and_returns_promptly() {
        let marker = marker_path("retained-release-failed");
        let permit = LaunchPermit::retained(|| {
            Ok(crate::LaunchOwnership::retained(|| {
                Ok(crate::ValidatedLaunchOwnership::retained(|| {
                    Err(HookError("injected post-gate unlock failure".into()))
                }))
            }))
        });
        let mut command = marker_command(
            &marker,
            "sleep 0.25; printf escaped > \"$1\"; sleep 5",
            permit,
        );
        command.return_stdin_to_caller_after_gate();
        let start = Instant::now();
        let error = command
            .spawn()
            .expect_err("post-gate ownership release failure must kill the runtime");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "a retained-mode release failure must return promptly, not hang behind an open pipe"
        );
        assert_eq!(error.phase(), SpawnPhase::Executed);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !marker.exists(),
            "runtime survived a failed post-gate ownership release in retained mode"
        );
    }

    #[test]
    fn watchdog_kills_a_runtime_blocked_reading_retained_stdin() {
        let marker = marker_path("retained-watchdog");
        let permit = LaunchPermit::retained(|| Ok(crate::LaunchOwnership::immediate()));
        // A longer deadline than `independent_boottime_deadline_kills_runtime_without_runner_
        // progress`'s 100ms -- that test only needs the deadline to beat a `sleep`, but this one
        // also needs the shell to reliably start and touch its marker before the deadline fires
        // (checked below), so a tighter window risks scheduler flakiness under load.
        let mut command =
            SandboxCommand::new("/bin/sh", Some(permit), Some(Duration::from_millis(750))).unwrap();
        command.return_stdin_to_caller_after_gate();
        command
            .command_mut()
            .arg("-c")
            // Blocks forever reading stdin (the caller below never writes or drops its handle) —
            // only the independent watchdog deadline can end this. Touches the marker up front so
            // a later existence check can't be confused with "the shell never even started".
            .arg("touch \"$1\"; cat > /dev/null")
            .arg("myelin-retained-watchdog-runtime")
            .arg(&marker)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().expect("durable commit releases runtime");
        let pipe = child
            .stdin()
            .take()
            .expect("retained mode must hand back a live stdin pipe");
        // Confirm the guest actually started (and is therefore genuinely blocked in `cat`, not
        // just slow to schedule) BEFORE relying on the watchdog to end it below.
        let started_deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() {
            assert!(
                Instant::now() < started_deadline,
                "the guest never even started"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        // Deliberately held open (not written to, not dropped) until after the exit check — proving
        // the watchdog, not stdin EOF, is what ends a runtime blocked reading a retained pipe.
        // `wait_for_exit`'s own internal 2-second deadline is the real proof here: it panics if the
        // guest never exits, which is exactly what would happen if the watchdog failed to fire
        // against a runtime blocked reading a pipe we deliberately never close.
        wait_for_exit(&mut child);
        drop(pipe);
        let _ = std::fs::remove_file(marker);
    }
}

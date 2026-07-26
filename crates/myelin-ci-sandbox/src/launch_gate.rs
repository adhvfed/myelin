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
        })
    }

    pub(crate) fn command_mut(&mut self) -> &mut Command {
        &mut self.command
    }

    pub(crate) fn is_fenced(&self) -> bool {
        self.fenced
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
                    SpawnFailure::uncommitted(format!(
                        "arm sandbox launch watchdog deadline: {error}"
                    ))
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
            SpawnFailure::uncommitted(format!("spawn sandbox process: {error}"))
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
                    let _ = child.wait();
                    return Err(SpawnFailure::uncommitted(
                        "launch guard gate pipe unavailable".to_string(),
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
                kill_process_group(group_id);
                let _ = child.wait();
                return Err(SpawnFailure::uncommitted(format!(
                    "launch guard failed to arm liveness watchdog: {error}"
                )));
            }
            let permit = self
                .permit
                .take()
                .expect("fenced sandbox command owns one launch permit");
            let ownership = match permit.commit() {
                Ok(ownership) => ownership,
                Err(error) => {
                    kill_process_group(group_id);
                    let _ = child.wait();
                    // The CAS returned an error, but that does NOT prove nothing committed: the
                    // durable store may have committed and lost the acknowledgement (e.g. a
                    // Postgres commit whose result never reached the caller). Calling this
                    // Uncommitted would let a caller release a reservation the store may still
                    // consider owned. Neither release nor settle — surface the ambiguity so the
                    // caller defers to durable reconciliation instead of guessing.
                    return Err(SpawnFailure::commit_outcome_unknown(format!(
                        "durable launch commit returned an error before sandbox exec: {error}"
                    )));
                }
            };
            let ownership = match ownership.validate() {
                Ok(ownership) => ownership,
                Err(error) => {
                    kill_process_group(group_id);
                    let _ = child.wait();
                    // The CAS DID commit (durably); the runtime is being killed before exec — the
                    // reservation's cost is real even though no workload ran.
                    return Err(SpawnFailure::committed_but_not_executed(format!(
                        "durable launch ownership was lost before sandbox exec: {error}"
                    )));
                }
            };
            // Captured IMMEDIATELY BEFORE the write below, not after: once the byte is written the
            // guard's `exec "$@"` could start at any moment, so this must be the TRUE release
            // moment a later Executed-phase failure, and the eventual successful `SandboxChild`,
            // report elapsed time from — never a later point after the (possibly slow) write
            // syscall or this whole call has returned.
            let release_at = Instant::now();
            if let Err(error) = gate.write_all(b"launch\n") {
                kill_process_group(group_id);
                let _ = child.wait();
                // Committed, but the guard never even read the release byte — still no exec. The
                // write failed, so `release_at` is discarded (never assigned to `executed_at`) —
                // this phase carries no execution timestamp.
                return Err(SpawnFailure::committed_but_not_executed(format!(
                    "release sandbox after durable launch commit: {error}"
                )));
            }
            executed_at = Some(release_at);
            drop(gate);
            if let Err(error) = ownership.release() {
                kill_process_group(group_id);
                let _ = child.wait();
                // The gate byte WAS written above — the guard's `exec "$@"` may already be
                // running by the time this specific release call failed. Treat as a real (if
                // botched) execution: the caller must account conservative fallback usage, never
                // charge zero for it.
                return Err(SpawnFailure::executed(
                    format!("release durable launch ownership after sandbox exec handoff: {error}"),
                    executed_at.expect("executed_at was just set above"),
                ));
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
#[derive(Debug)]
pub(crate) struct SpawnFailure {
    phase: SpawnPhase,
    message: String,
    executed_at: Option<Instant>,
}

impl SpawnFailure {
    fn uncommitted(message: String) -> Self {
        Self {
            phase: SpawnPhase::Uncommitted,
            message,
            executed_at: None,
        }
    }

    fn commit_outcome_unknown(message: String) -> Self {
        Self {
            phase: SpawnPhase::CommitOutcomeUnknown,
            message,
            executed_at: None,
        }
    }

    fn committed_but_not_executed(message: String) -> Self {
        Self {
            phase: SpawnPhase::CommittedButNotExecuted,
            message,
            executed_at: None,
        }
    }

    fn executed(message: String, executed_at: Instant) -> Self {
        Self {
            phase: SpawnPhase::Executed,
            message,
            executed_at: Some(executed_at),
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

    pub(crate) fn kill_and_wait(&mut self) {
        if let Some(group_id) = self.process_group {
            kill_process_group(group_id);
        } else {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        self.finish_liveness();
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
}

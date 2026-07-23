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
use std::time::Duration;

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
    pub(crate) fn spawn(mut self) -> Result<SandboxChild, String> {
        let watchdog_timer = if self.fenced {
            Some(
                boottime_timer(
                    self.watchdog_timeout
                        .expect("fenced sandbox command owns a watchdog deadline"),
                )
                .map_err(|error| format!("arm sandbox launch watchdog deadline: {error}"))?,
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
        let mut child = self
            .command
            .spawn()
            .map_err(|error| format!("spawn sandbox process: {error}"))?;
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
                    return Err("launch guard gate pipe unavailable".to_string());
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
                return Err(format!(
                    "launch guard failed to arm liveness watchdog: {error}"
                ));
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
                    return Err(format!(
                        "durable launch commit failed before sandbox exec: {error}"
                    ));
                }
            };
            let ownership = match ownership.validate() {
                Ok(ownership) => ownership,
                Err(error) => {
                    kill_process_group(group_id);
                    let _ = child.wait();
                    return Err(format!(
                        "durable launch ownership was lost before sandbox exec: {error}"
                    ));
                }
            };
            if let Err(error) = gate.write_all(b"launch\n") {
                kill_process_group(group_id);
                let _ = child.wait();
                return Err(format!(
                    "release sandbox after durable launch commit: {error}"
                ));
            }
            drop(gate);
            if let Err(error) = ownership.release() {
                kill_process_group(group_id);
                let _ = child.wait();
                return Err(format!(
                    "release durable launch ownership after sandbox exec handoff: {error}"
                ));
            }
        }

        let process_group = self.fenced.then_some(child.id() as i32);
        Ok(SandboxChild {
            child,
            liveness_write: self.liveness_write,
            watchdog_timer,
            process_group,
        })
    }
}

/// Child returned by [`SandboxCommand`]. Closing `liveness_write` after the process leader exits
/// releases the watchdog, which removes surviving descendants and then kills itself.
#[derive(Debug)]
pub(crate) struct SandboxChild {
    child: Child,
    liveness_write: Option<OwnedFd>,
    watchdog_timer: Option<OwnedFd>,
    process_group: Option<i32>,
}

impl SandboxChild {
    pub(crate) fn id(&self) -> u32 {
        self.child.id()
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
    let _owned_child = command.spawn()?;
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
        assert!(error.contains("injected commit failure"));
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
        assert!(error.contains("injected session ownership loss"));
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
        assert!(error.contains("injected post-gate unlock failure"));
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

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostGateStdin {
    Close,
    ReturnToCaller,
}

pub(crate) struct SandboxCommand {
    command: Command,
    fence: LaunchFence,
}

enum LaunchFence {
    Unfenced,
    Fenced(FencedLaunch),
}

struct FencedLaunch {
    permit: LaunchPermit,
    liveness_read: OwnedFd,
    liveness_write: OwnedFd,
    ready_read: OwnedFd,
    ready_write: OwnedFd,
    cgroup_kill: Option<File>,
    watchdog_timeout: Duration,
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
                fence: LaunchFence::Unfenced,
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
            fence: LaunchFence::Fenced(FencedLaunch {
                permit,
                liveness_read,
                liveness_write,
                ready_read,
                ready_write,
                cgroup_kill: None,
                watchdog_timeout,
                post_gate_stdin: PostGateStdin::Close,
            }),
        })
    }

    pub(crate) fn command_mut(&mut self) -> &mut Command {
        &mut self.command
    }

    pub(crate) fn is_fenced(&self) -> bool {
        matches!(self.fence, LaunchFence::Fenced(_))
    }

    pub(crate) fn return_stdin_to_caller_after_gate(&mut self) -> io::Result<()> {
        match &mut self.fence {
            LaunchFence::Fenced(fence) => {
                fence.post_gate_stdin = PostGateStdin::ReturnToCaller;
                Ok(())
            }
            LaunchFence::Unfenced => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only a fenced sandbox command has a launch-gate pipe to retain",
            )),
        }
    }

    pub(crate) fn kill_cgroup_on_liveness_loss(&mut self, kill_file: File) -> io::Result<()> {
        match &mut self.fence {
            LaunchFence::Fenced(fence) => {
                fence.cgroup_kill = Some(kill_file);
                Ok(())
            }
            LaunchFence::Unfenced => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only a fenced sandbox command has a liveness watchdog",
            )),
        }
    }

    pub(crate) fn spawn(self) -> Result<SandboxChild, SpawnFailure> {
        match self.fence {
            LaunchFence::Unfenced => spawn_unfenced(self.command),
            LaunchFence::Fenced(fence) => spawn_fenced(self.command, fence),
        }
    }
}

fn spawn_unfenced(mut command: Command) -> Result<SandboxChild, SpawnFailure> {
    let executed_at = Instant::now();
    let child = command.spawn().map_err(|error| {
        SpawnFailure::uncommitted(
            format!("spawn sandbox process: {error}"),
            DirectChildRetirement::NoChildReturned,
        )
    })?;
    Ok(SandboxChild {
        child,
        liveness_write: None,
        watchdog_timer: None,
        process_group: None,
        executed_at,
    })
}

fn spawn_fenced(mut command: Command, fence: FencedLaunch) -> Result<SandboxChild, SpawnFailure> {
    let FencedLaunch {
        permit,
        liveness_read,
        liveness_write,
        mut ready_read,
        ready_write,
        cgroup_kill,
        watchdog_timeout,
        post_gate_stdin,
    } = fence;
    let watchdog_timer = boottime_timer(watchdog_timeout).map_err(|error| {
        SpawnFailure::uncommitted(
            format!("arm sandbox launch watchdog deadline: {error}"),
            DirectChildRetirement::NoChildReturned,
        )
    })?;
    let liveness_fd = liveness_read.as_raw_fd();
    let liveness_write_fd = liveness_write.as_raw_fd();
    let ready_fd = ready_write.as_raw_fd();
    let timer_fd = watchdog_timer.as_raw_fd();
    let cgroup_kill_fd = cgroup_kill.as_ref().map_or(-1, AsRawFd::as_raw_fd);
    unsafe {
        command.pre_exec(move || {
            if libc::setpgid(0, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            let process_group = libc::getpid();
            let watchdog = libc::fork();
            if watchdog == -1 {
                return Err(io::Error::last_os_error());
            }
            if watchdog == 0 {
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
    let mut child = command.spawn().map_err(|error| {
        SpawnFailure::uncommitted(
            format!("spawn sandbox process: {error}"),
            DirectChildRetirement::NoChildReturned,
        )
    })?;
    drop(liveness_read);
    drop(ready_write);
    drop(cgroup_kill);

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
    let ready_result = wait_until_ready(&mut ready_read);
    drop(ready_read);
    if let Err(error) = ready_result {
        drop(gate);
        kill_process_group(group_id);
        let child_retirement = DirectChildRetirement::from_wait_result(child.wait());
        return Err(SpawnFailure::uncommitted(
            format!("launch guard failed to arm liveness watchdog: {error}"),
            child_retirement,
        ));
    }
    let ownership = match permit.commit() {
        Ok(ownership) => ownership,
        Err(error) => {
            drop(gate);
            kill_process_group(group_id);
            let child_retirement = DirectChildRetirement::from_wait_result(child.wait());
            return Err(SpawnFailure::commit_outcome_unknown(
                format!("durable launch commit returned an error before sandbox exec: {error}"),
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
            return Err(SpawnFailure::committed_but_not_executed(
                format!("durable launch ownership was lost before sandbox exec: {error}"),
                child_retirement,
            ));
        }
    };
    let executed_at = Instant::now();
    if let Err(error) = gate.write_all(b"launch\n") {
        drop(gate);
        kill_process_group(group_id);
        let child_retirement = DirectChildRetirement::from_wait_result(child.wait());
        return Err(SpawnFailure::committed_but_not_executed(
            format!("release sandbox after durable launch commit: {error}"),
            child_retirement,
        ));
    }
    match post_gate_stdin {
        PostGateStdin::Close => {
            drop(gate);
            if let Err(error) = ownership.release() {
                kill_process_group(group_id);
                let child_retirement = DirectChildRetirement::from_wait_result(child.wait());
                return Err(SpawnFailure::executed(
                    format!("release durable launch ownership after sandbox exec handoff: {error}"),
                    executed_at,
                    child_retirement,
                ));
            }
        }
        PostGateStdin::ReturnToCaller => match ownership.release() {
            Ok(()) => child.stdin = Some(gate),
            Err(error) => {
                drop(gate);
                kill_process_group(group_id);
                let child_retirement = DirectChildRetirement::from_wait_result(child.wait());
                return Err(SpawnFailure::executed(
                    format!("release durable launch ownership after sandbox exec handoff: {error}"),
                    executed_at,
                    child_retirement,
                ));
            }
        },
    }
    Ok(SandboxChild {
        child,
        liveness_write: Some(liveness_write),
        watchdog_timer: Some(watchdog_timer),
        process_group: Some(group_id),
        executed_at,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectChildRetirement {
    NoChildReturned,
    Reaped,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpawnPhase {
    Uncommitted,
    CommitOutcomeUnknown,
    CommittedButNotExecuted,
    Executed,
}

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

    pub(crate) fn executed_at(&self) -> Option<Instant> {
        self.executed_at
    }

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
        unsafe { libc::poll(&mut poll_fd, 1, 0) == 1 && poll_fd.revents & libc::POLLIN != 0 }
    }

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
        unsafe {
            libc::kill(-group_id, libc::SIGKILL);
        }
    }
}

fn boottime_timer(timeout: Duration) -> io::Result<OwnedFd> {
    let fd = unsafe { libc::timerfd_create(libc::CLOCK_BOOTTIME, libc::TFD_CLOEXEC) };
    if fd == -1 {
        return Err(io::Error::last_os_error());
    }
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
    if unsafe { libc::timerfd_settime(owned.as_raw_fd(), 0, &value, std::ptr::null_mut()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(owned)
}

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
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn wait_until_ready(pipe: &mut OwnedFd) -> io::Result<()> {
    let mut poll_fd = libc::pollfd {
        fd: pipe.as_raw_fd(),
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    };
    loop {
        let result = unsafe { libc::poll(&mut poll_fd, 1, 5_000) };
        if result > 0 {
            let mut ready = [0u8; 1];
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

        child.finish_liveness();
        wait_for_exit(&mut child);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !marker.exists(),
            "runtime survived loss of its owning runner process"
        );
    }

    #[test]
    fn default_fenced_mode_closes_stdin_and_delivers_immediate_eof() {
        let marker = marker_path("default-close-eof");
        let permit = LaunchPermit::retained(|| Ok(crate::LaunchOwnership::immediate()));
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
        command.return_stdin_to_caller_after_gate().unwrap();
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
        command.return_stdin_to_caller_after_gate().unwrap();
        let mut child = command.spawn().expect("durable commit releases runtime");
        let mut pipe = child
            .stdin()
            .take()
            .expect("retained mode must hand back a live stdin pipe");
        pipe.write_all(payload)
            .expect("writing the request payload must succeed");
        drop(pipe);
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
        command.return_stdin_to_caller_after_gate().unwrap();
        let start = Instant::now();
        let error = command.spawn().expect_err(
            "lost post-commit ownership must refuse gate release even in retained mode",
        );
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
        command.return_stdin_to_caller_after_gate().unwrap();
        let error = command
            .spawn()
            .expect_err("failed commit must refuse launch even in retained mode");
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
        command.return_stdin_to_caller_after_gate().unwrap();
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
        let mut command =
            SandboxCommand::new("/bin/sh", Some(permit), Some(Duration::from_millis(750))).unwrap();
        command.return_stdin_to_caller_after_gate().unwrap();
        command
            .command_mut()
            .arg("-c")
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
        let started_deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() {
            assert!(
                Instant::now() < started_deadline,
                "the guest never even started"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        wait_for_exit(&mut child);
        drop(pipe);
        let _ = std::fs::remove_file(marker);
    }
}

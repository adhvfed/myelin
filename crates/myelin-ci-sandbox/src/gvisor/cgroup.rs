use std::io;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use super::unique_suffix;

pub struct MemoryCgroup {
    dir: PathBuf,
    identity: (u64, u64),
    active: std::cell::Cell<bool>,
}

fn resource_cgroup_refusal(reason: impl core::fmt::Display) -> String {
    format!(
        "{reason} - refusing to run the gVisor workload without its hard resource bounds \
         (SI-017 fail-closed)"
    )
}

const CPU_MAX_PERIOD_US: u64 = 100_000;
const CPU_MAX_MIN_QUOTA_US: u64 = 1_000;

fn cpu_max_value(cpu_millis: u32) -> Result<String, String> {
    if cpu_millis == 0 {
        return Err("cpu_millis is zero; cannot establish a nonzero cpu.max quota".to_string());
    }
    let quota_us = u64::from(cpu_millis)
        .checked_mul(CPU_MAX_PERIOD_US)
        .and_then(|value| value.checked_div(1000))
        .ok_or_else(|| format!("cpu.max quota overflow for cpu_millis={cpu_millis}"))?
        .max(CPU_MAX_MIN_QUOTA_US);
    Ok(format!("{quota_us} {CPU_MAX_PERIOD_US}"))
}

fn write_job_cgroup_limits_given<F>(
    dir: &Path,
    mem_bytes: u64,
    cpu_millis: u32,
    mut write: F,
) -> Result<(), String>
where
    F: FnMut(&Path, &[u8]) -> io::Result<()>,
{
    let memory_max = mem_bytes.to_string();
    write(dir.join("memory.max").as_path(), memory_max.as_bytes())
        .map_err(|e| format!("write memory.max={mem_bytes} to {dir:?}: {e}"))?;

    write(dir.join("memory.swap.max").as_path(), b"0")
        .map_err(|e| format!("write memory.swap.max=0 to {dir:?}: {e}"))?;

    let cpu_max = cpu_max_value(cpu_millis)?;
    write(dir.join("cpu.max").as_path(), cpu_max.as_bytes())
        .map_err(|e| format!("write cpu.max={cpu_max} to {dir:?}: {e}"))?;
    Ok(())
}

fn cgroup_identity(dir: &Path) -> io::Result<(u64, u64)> {
    let meta = std::fs::metadata(dir)?;
    Ok((meta.dev(), meta.ino()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityCheckOutcome {
    Proceed,
    Disarm,
    LeaveArmed,
}

fn classify_identity_check(
    expected: (u64, u64),
    result: &io::Result<(u64, u64)>,
) -> IdentityCheckOutcome {
    match result {
        Ok(current) if *current == expected => IdentityCheckOutcome::Proceed,
        Ok(_) => IdentityCheckOutcome::Disarm,
        Err(e) if e.kind() == io::ErrorKind::NotFound => IdentityCheckOutcome::Disarm,
        Err(_) => IdentityCheckOutcome::LeaveArmed,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CgroupQuiescenceEvidence {
    cgroup_identity: (u64, u64),
}

impl CgroupQuiescenceEvidence {
    pub fn cgroup_identity(&self) -> (u64, u64) {
        self.cgroup_identity
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn assert_for_tests(cgroup_identity: (u64, u64)) -> Self {
        CgroupQuiescenceEvidence { cgroup_identity }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CgroupQuiescenceError {
    IdentityUnreadable(String),
    IdentityChanged {
        expected: (u64, u64),
        actual: (u64, u64),
    },
    Kill(String),
    EventsUnreadable(String),
    EventsMalformed(String),
    StillPopulated {
        waited: Duration,
    },
    Remove(String),
}

impl std::fmt::Display for CgroupQuiescenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CgroupQuiescenceError::IdentityUnreadable(e) => {
                write!(
                    f,
                    "failed to stat the memory cgroup to revalidate its identity: {e}"
                )
            }
            CgroupQuiescenceError::IdentityChanged { expected, actual } => write!(
                f,
                "the memory cgroup's path no longer names the cgroup it was created as (expected \
                 identity {expected:?}, found {actual:?})"
            ),
            CgroupQuiescenceError::Kill(e) => {
                write!(f, "failed to write cgroup.kill: {e}")
            }
            CgroupQuiescenceError::EventsUnreadable(e) => {
                write!(f, "failed to read cgroup.events: {e}")
            }
            CgroupQuiescenceError::EventsMalformed(e) => {
                write!(f, "cgroup.events content was malformed: {e}")
            }
            CgroupQuiescenceError::StillPopulated { waited } => write!(
                f,
                "cgroup.events still reported populated=1 after waiting {waited:?}"
            ),
            CgroupQuiescenceError::Remove(e) => {
                write!(
                    f,
                    "cgroup.events reported populated=0, but rmdir failed: {e}"
                )
            }
        }
    }
}

impl std::error::Error for CgroupQuiescenceError {}

fn parse_cgroup_events_populated(content: &str) -> Result<bool, String> {
    let mut found = None;
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else { continue };
        if key != "populated" {
            continue;
        }
        let Some(value) = parts.next() else {
            return Err(format!("`populated` line has no value: {line:?}"));
        };
        if parts.next().is_some() {
            return Err(format!("`populated` line has extra fields: {line:?}"));
        }
        let parsed = match value {
            "0" => false,
            "1" => true,
            other => return Err(format!("`populated` value is not 0/1: {other:?}")),
        };
        if found.replace(parsed).is_some() {
            return Err("`populated` key appears more than once".to_string());
        }
    }
    found.ok_or_else(|| "no `populated` key found".to_string())
}

fn wait_for_unpopulated(
    events_path: &Path,
    deadline: Duration,
) -> Result<(), CgroupQuiescenceError> {
    let start = Instant::now();
    loop {
        let content = std::fs::read_to_string(events_path)
            .map_err(|e| CgroupQuiescenceError::EventsUnreadable(e.to_string()))?;
        let populated = parse_cgroup_events_populated(&content)
            .map_err(CgroupQuiescenceError::EventsMalformed)?;
        if !populated {
            return Ok(());
        }
        let elapsed = start.elapsed();
        if elapsed >= deadline {
            return Err(CgroupQuiescenceError::StillPopulated { waited: elapsed });
        }
        std::thread::sleep((deadline - elapsed).min(Duration::from_millis(20)));
    }
}

impl MemoryCgroup {
    pub fn create(mem_bytes: u64, cpu_millis: u32) -> Result<MemoryCgroup, String> {
        const ROOT: &str = "/sys/fs/cgroup";
        let content = std::fs::read_to_string("/proc/self/cgroup")
            .map_err(|e| resource_cgroup_refusal(format!("read /proc/self/cgroup: {e}")))?;
        let rel = content
            .lines()
            .find_map(|l| l.strip_prefix("0::"))
            .map(str::trim)
            .ok_or_else(|| {
                resource_cgroup_refusal(
                    "no cgroup v2 unified hierarchy (`0::` line absent); cannot establish a job cgroup",
                )
            })?;
        let our_dir = PathBuf::from(ROOT).join(rel.trim_start_matches('/'));
        let controllers =
            std::fs::read_to_string(our_dir.join("cgroup.controllers")).unwrap_or_default();
        for required in ["memory", "cpu"] {
            if !controllers.split_whitespace().any(|c| c == required) {
                return Err(resource_cgroup_refusal(format!(
                    "the `{required}` cgroup controller is NOT delegated to {our_dir:?} \
                     (controllers: {controllers:?}); cannot bound gVisor resources"
                )));
            }
        }
        let parent = our_dir.parent().ok_or_else(|| {
            resource_cgroup_refusal(
                "this process's cgroup has no parent; cannot create a sibling job cgroup",
            )
        })?;
        let dir = parent.join(format!(
            "myelin-job-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = std::fs::remove_dir(&dir);
        std::fs::create_dir(&dir)
            .map_err(|e| resource_cgroup_refusal(format!("create job cgroup {dir:?}: {e}")))?;
        let cg_controllers =
            std::fs::read_to_string(dir.join("cgroup.controllers")).unwrap_or_default();
        for required in ["memory", "cpu"] {
            if !cg_controllers.split_whitespace().any(|c| c == required) {
                let _ = std::fs::remove_dir(&dir);
                return Err(resource_cgroup_refusal(format!(
                    "the created cgroup {dir:?} has no `{required}` controller \
                     (parent did not delegate it)"
                )));
            }
        }
        if let Err(e) = write_job_cgroup_limits_given(&dir, mem_bytes, cpu_millis, |path, value| {
            std::fs::write(path, value)
        }) {
            let _ = std::fs::remove_dir(&dir);
            return Err(resource_cgroup_refusal(e));
        }
        let identity = cgroup_identity(&dir).map_err(|e| {
            let _ = std::fs::remove_dir(&dir);
            resource_cgroup_refusal(format!("stat freshly-created job cgroup {dir:?}: {e}"))
        })?;
        Ok(MemoryCgroup {
            dir,
            identity,
            active: std::cell::Cell::new(true),
        })
    }

    pub fn identity(&self) -> (u64, u64) {
        self.identity
    }

    #[cfg(all(test, feature = "test-support"))]
    pub(super) fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn place_child(&self, cmd: &mut Command) -> std::io::Result<()> {
        let procs = std::fs::OpenOptions::new()
            .write(true)
            .open(self.dir.join("cgroup.procs"))?;
        unsafe {
            cmd.pre_exec(move || {
                let mut pid = std::process::id();
                let mut buf = [0u8; 11];
                let mut i = buf.len();
                i -= 1;
                buf[i] = b'\n';
                if pid == 0 {
                    i -= 1;
                    buf[i] = b'0';
                } else {
                    while pid > 0 {
                        i -= 1;
                        buf[i] = b'0' + (pid % 10) as u8;
                        pid /= 10;
                    }
                }
                (&procs).write_all(&buf[i..])
            });
        }
        Ok(())
    }

    pub(super) fn kill_file(&self) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new()
            .write(true)
            .open(self.dir.join("cgroup.kill"))
    }

    pub fn cleanup(&self) {
        if !self.active.get() {
            return;
        }
        match classify_identity_check(self.identity, &cgroup_identity(&self.dir)) {
            IdentityCheckOutcome::Proceed => {}
            IdentityCheckOutcome::Disarm => {
                self.active.set(false);
                return;
            }
            IdentityCheckOutcome::LeaveArmed => return,
        }
        let _ = std::fs::write(self.dir.join("cgroup.kill"), b"1");
        for _ in 0..200 {
            match std::fs::read_to_string(self.dir.join("cgroup.procs")) {
                Ok(s) if s.split_whitespace().next().is_none() => break,
                Ok(_) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
        match std::fs::remove_dir(&self.dir) {
            Ok(()) => self.active.set(false),
            Err(e) if e.kind() == io::ErrorKind::NotFound => self.active.set(false),
            Err(_) => {}
        }
    }

    pub fn quiesce(
        self,
        timeout: Duration,
    ) -> Result<CgroupQuiescenceEvidence, CgroupQuiescenceError> {
        let current_identity = cgroup_identity(&self.dir)
            .map_err(|e| CgroupQuiescenceError::IdentityUnreadable(e.to_string()))?;
        if current_identity != self.identity {
            self.active.set(false);
            return Err(CgroupQuiescenceError::IdentityChanged {
                expected: self.identity,
                actual: current_identity,
            });
        }
        std::fs::write(self.dir.join("cgroup.kill"), b"1")
            .map_err(|e| CgroupQuiescenceError::Kill(e.to_string()))?;
        wait_for_unpopulated(&self.dir.join("cgroup.events"), timeout)?;
        std::fs::remove_dir(&self.dir).map_err(|e| CgroupQuiescenceError::Remove(e.to_string()))?;
        self.active.set(false);
        Ok(CgroupQuiescenceEvidence {
            cgroup_identity: self.identity,
        })
    }
}

impl Drop for MemoryCgroup {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "test-support")]
    use crate::launch_gate::SandboxCommand;
    #[cfg(feature = "test-support")]
    use crate::LaunchPermit;
    #[cfg(feature = "test-support")]
    use std::process::Stdio;

    #[test]
    fn job_cgroup_limits_are_written_together_from_policy() {
        let dir = PathBuf::from("/deterministic/job-cgroup");
        let mut writes = Vec::<(PathBuf, Vec<u8>)>::new();
        write_job_cgroup_limits_given(&dir, 64 << 20, 2000, |path, value| {
            writes.push((path.to_path_buf(), value.to_vec()));
            Ok(())
        })
        .expect("all resource controls should be installed");

        assert_eq!(
            writes,
            vec![
                (
                    dir.join("memory.max"),
                    (64u64 << 20).to_string().into_bytes()
                ),
                (dir.join("memory.swap.max"), b"0".to_vec()),
                (dir.join("cpu.max"), b"200000 100000".to_vec()),
            ],
            "2000 millicpu must become a two-core cpu.max quota in the same job cgroup"
        );

    }

    #[test]
    fn cpu_max_quota_floors_sub_ten_millicpu_at_kernel_minimum() {
        for cpu_millis in 1..=9 {
            assert_eq!(
                cpu_max_value(cpu_millis).expect("positive millicpu must produce cpu.max"),
                "1000 100000",
                "{cpu_millis} millicpu must use the kernel's minimum accepted quota"
            );
        }
    }

    #[test]
    fn cpu_max_write_failure_aborts_cgroup_setup_fail_closed() {
        let dir = PathBuf::from("/deterministic/job-cgroup");
        let mut attempted = Vec::<PathBuf>::new();
        let error = write_job_cgroup_limits_given(&dir, 64 << 20, 2000, |path, _| {
            attempted.push(path.to_path_buf());
            if path == dir.join("cpu.max") {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected cpu.max denial",
                ))
            } else {
                Ok(())
            }
        })
        .expect_err("a cpu.max write failure must abort cgroup setup");

        assert_eq!(attempted.last(), Some(&dir.join("cpu.max")));
        assert!(error.contains("write cpu.max=200000 100000"));
        assert!(error.contains("injected cpu.max denial"));
    }

    #[test]
    fn swap_max_write_failure_aborts_cgroup_setup_fail_closed() {
        let dir = PathBuf::from("/deterministic/job-cgroup");
        let mut attempted = Vec::<PathBuf>::new();
        let error = write_job_cgroup_limits_given(&dir, 64 << 20, 2000, |path, _| {
            attempted.push(path.to_path_buf());
            if path == dir.join("memory.swap.max") {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected swap.max denial",
                ))
            } else {
                Ok(())
            }
        })
        .expect_err("a memory.swap.max write failure must abort cgroup setup");

        assert_eq!(
            attempted,
            vec![dir.join("memory.max"), dir.join("memory.swap.max")],
            "fail immediately at swap.max; cpu.max and launch must never be reached"
        );
        assert!(error.contains("write memory.swap.max=0"));
        assert!(error.contains("injected swap.max denial"));
    }

    #[test]
    fn memory_cgroup_round_trips_or_fails_closed() {
        match MemoryCgroup::create(64 << 20, 1000) {
            Ok(cg) => {
                let dir = cg.dir.clone();
                let max = std::fs::read_to_string(dir.join("memory.max")).unwrap_or_default();
                assert_eq!(
                    max.trim(),
                    (64u64 << 20).to_string(),
                    "memory.max must be written to the cap (the REAL out-of-band bound)"
                );
                assert!(
                    std::fs::read_to_string(dir.join("cgroup.controllers"))
                        .unwrap_or_default()
                        .split_whitespace()
                        .any(|c| c == "memory"),
                    "the created cgroup must actually carry the memory controller"
                );
                cg.cleanup();
                assert!(!dir.exists(), "cleanup must rmdir the cgroup (no leaks)");
            }
            Err(e) => {
                assert!(
                    e.contains("fail-closed"),
                    "an unestablishable memory cgroup must fail CLOSED with a clear reason, got: {e}"
                );
            }
        }
    }

    #[test]
    fn parse_cgroup_events_populated_accepts_reordered_and_extra_keys() {
        assert_eq!(
            parse_cgroup_events_populated("populated 0\nfrozen 0\n"),
            Ok(false)
        );
        assert_eq!(
            parse_cgroup_events_populated("frozen 1\npopulated 1\n"),
            Ok(true)
        );
        assert_eq!(
            parse_cgroup_events_populated("some_future_key 42\npopulated 0\nfrozen 0\n"),
            Ok(false),
            "unrecognized keys must not be treated as malformed - the kernel's format is not a \
             fixed schema"
        );
    }

    #[test]
    fn parse_cgroup_events_populated_rejects_a_missing_key() {
        assert!(parse_cgroup_events_populated("frozen 0\n").is_err());
    }

    #[test]
    fn parse_cgroup_events_populated_rejects_a_duplicate_key() {
        assert!(parse_cgroup_events_populated("populated 0\npopulated 1\n").is_err());
    }

    #[test]
    fn parse_cgroup_events_populated_rejects_a_non_bit_value() {
        assert!(parse_cgroup_events_populated("populated 2\n").is_err());
    }

    #[test]
    fn parse_cgroup_events_populated_rejects_extra_fields_on_the_line() {
        assert!(parse_cgroup_events_populated("populated 0 extra\n").is_err());
    }

    #[test]
    fn wait_for_unpopulated_times_out_on_a_permanently_populated_fixture() {
        let path = std::env::temp_dir().join(format!(
            "myelin-cgroup-events-fixture-populated-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::write(&path, b"populated 1\nfrozen 0\n").unwrap();
        let result = wait_for_unpopulated(&path, Duration::from_millis(30));
        assert!(matches!(
            result,
            Err(CgroupQuiescenceError::StillPopulated { .. })
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wait_for_unpopulated_performs_exactly_one_observation_at_a_zero_deadline() {
        let path = std::env::temp_dir().join(format!(
            "myelin-cgroup-events-fixture-zero-deadline-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::write(&path, b"populated 0\n").unwrap();
        assert_eq!(wait_for_unpopulated(&path, Duration::ZERO), Ok(()));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wait_for_unpopulated_surfaces_an_unreadable_events_file() {
        let path = std::env::temp_dir().join(format!(
            "myelin-cgroup-events-fixture-missing-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let result = wait_for_unpopulated(&path, Duration::from_millis(10));
        assert!(matches!(
            result,
            Err(CgroupQuiescenceError::EventsUnreadable(_))
        ));
    }

    #[test]
    fn wait_for_unpopulated_surfaces_malformed_events_content() {
        let path = std::env::temp_dir().join(format!(
            "myelin-cgroup-events-fixture-malformed-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::write(&path, b"not a valid cgroup.events file\n").unwrap();
        let result = wait_for_unpopulated(&path, Duration::from_millis(10));
        assert!(matches!(
            result,
            Err(CgroupQuiescenceError::EventsMalformed(_))
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cgroup_quiescence_evidence_assert_for_tests_round_trips() {
        let evidence = CgroupQuiescenceEvidence::assert_for_tests((7, 8));
        assert_eq!(evidence.cgroup_identity(), (7, 8));
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn quiesce_succeeds_on_an_empty_real_cgroup_and_removes_it() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir.clone();
        let identity = cg.identity();
        let evidence = cg
            .quiesce(Duration::from_millis(500))
            .expect("an empty cgroup with no processes ever placed must quiesce cleanly");
        assert_eq!(evidence.cgroup_identity(), identity);
        assert!(!dir.exists(), "quiesce must rmdir the cgroup on success");
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn quiesce_kills_a_descendant_detached_outside_the_runtime_process_group() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir.clone();
        let armed = std::env::temp_dir().join(format!(
            "myelin-gvisor-quiesce-armed-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let escaped = std::env::temp_dir().join(format!(
            "myelin-gvisor-quiesce-escaped-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(
                "setsid /bin/sh -c 'printf \"%s\" \"$$\" > \"$1\"; \
                 sleep 0.35; printf escaped > \"$2\"; sleep 5' \
                 myelin-detached \"$1\" \"$2\" & sleep 5",
            )
            .arg("myelin-quiesce-detached")
            .arg(&armed)
            .arg(&escaped)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cg.place_child(&mut command)
            .expect("place the readiness process in the real gVisor cgroup");
        let mut child = command.spawn().expect("release detached-descendant probe");
        let deadline = Instant::now() + Duration::from_secs(1);
        while !armed.exists() {
            assert!(
                Instant::now() < deadline,
                "detached cgroup descendant did not publish readiness"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let evidence = cg.quiesce(Duration::from_secs(2)).expect(
            "quiesce must kill every cgroup member (including the detached descendant) \
                     and observe populated=0",
        );
        assert!(!dir.exists());
        std::thread::sleep(Duration::from_millis(450));
        assert!(
            !escaped.exists(),
            "a descendant outside the runtime process group survived quiesce()'s cgroup.kill"
        );
        let _ = evidence;
        let _ = child.wait();
        let _ = std::fs::remove_file(armed);
        let _ = std::fs::remove_file(escaped);
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn quiesce_succeeds_without_the_caller_ever_reaping_a_killed_direct_child() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir.clone();
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("sleep 30")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cg.place_child(&mut command)
            .expect("place the direct child in the real gVisor cgroup");
        let mut child = command.spawn().expect("spawn the direct child");
        let evidence = cg
            .quiesce(Duration::from_secs(2))
            .expect("quiesce must kill and observe quiescence without the caller reaping first");
        assert!(!dir.exists());
        let _ = evidence;
        let _ = child.wait();
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn quiesce_refuses_to_mint_evidence_when_an_unexpected_child_cgroup_blocks_removal() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir.clone();
        let nested = dir.join("myelin-unexpected-child");
        std::fs::create_dir(&nested).expect("create an unexpected nested cgroup");
        let result = cg.quiesce(Duration::from_millis(500));
        assert!(
            matches!(result, Err(CgroupQuiescenceError::Remove(_))),
            "expected Remove, got {result:?}"
        );
        assert!(
            dir.exists(),
            "a refused removal must not silently vanish the cgroup"
        );
        let _ = std::fs::remove_dir(&nested);
        let _ = std::fs::remove_dir(&dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn drop_retries_removal_after_an_earlier_failed_cleanup_call() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir.clone();
        let nested = dir.join("myelin-unexpected-child");
        std::fs::create_dir(&nested).expect("create an unexpected nested cgroup");

        cg.cleanup();
        assert!(
            dir.exists(),
            "the blocked cleanup must not have vanished the cgroup"
        );

        std::fs::remove_dir(&nested).expect("clear the way for the retry");
        drop(cg);
        assert!(
            !dir.exists(),
            "Drop must retry removal after an earlier failed explicit cleanup() - a disarmed \
             handle would have leaked this cgroup"
        );
    }

    #[test]
    fn classify_identity_check_proceeds_when_identity_matches() {
        assert_eq!(
            classify_identity_check((1, 2), &Ok((1, 2))),
            IdentityCheckOutcome::Proceed
        );
    }

    #[test]
    fn classify_identity_check_disarms_on_a_confirmed_replacement() {
        assert_eq!(
            classify_identity_check((1, 2), &Ok((3, 4))),
            IdentityCheckOutcome::Disarm
        );
    }

    #[test]
    fn classify_identity_check_disarms_on_a_confirmed_absence() {
        assert_eq!(
            classify_identity_check((1, 2), &Err(io::Error::from(io::ErrorKind::NotFound))),
            IdentityCheckOutcome::Disarm
        );
    }

    #[test]
    fn classify_identity_check_leaves_armed_on_a_transient_error() {
        assert_eq!(
            classify_identity_check(
                (1, 2),
                &Err(io::Error::from(io::ErrorKind::PermissionDenied))
            ),
            IdentityCheckOutcome::LeaveArmed
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn quiesce_detects_an_identity_change_and_refuses_to_mint_evidence() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir.clone();
        let original_identity = cg.identity();
        std::fs::remove_dir(&dir).unwrap();
        std::fs::create_dir(&dir).unwrap();
        let mut replacement_command = Command::new("/bin/sh");
        replacement_command
            .arg("-c")
            .arg("sleep 30")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cg.place_child(&mut replacement_command)
            .expect("place a live process in the replacement cgroup");
        let mut replacement_child = replacement_command
            .spawn()
            .expect("spawn the replacement cgroup's own controlled member");

        let result = cg.quiesce(Duration::from_millis(500));
        match result {
            Err(CgroupQuiescenceError::IdentityChanged { expected, actual }) => {
                assert_eq!(expected, original_identity);
                assert_ne!(actual, original_identity);
            }
            other => panic!("expected IdentityChanged, got {other:?}"),
        }
        assert!(
            dir.exists(),
            "an identity mismatch must never let Drop remove the replacement cgroup"
        );
        assert!(
            replacement_child
                .try_wait()
                .expect("waitpid on the replacement's controlled member")
                .is_none(),
            "an identity mismatch must never let Drop kill the replacement cgroup's own process"
        );
        let _ = replacement_child.kill();
        let _ = replacement_child.wait();
        let _ = std::fs::write(dir.join("cgroup.kill"), b"1");
        for _ in 0..200 {
            match std::fs::read_to_string(dir.join("cgroup.procs")) {
                Ok(s) if s.split_whitespace().next().is_none() => break,
                Ok(_) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
        let _ = std::fs::remove_dir(&dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn launch_watchdog_cgroup_kills_a_descendant_outside_the_runtime_process_group() {
        use std::time::Instant;
        let cgroup = MemoryCgroup::create(64 << 20, 1000)
            .expect("the all-feature gVisor cgroup watchdog gate requires a real delegated cgroup");
        let armed = std::env::temp_dir().join(format!(
            "myelin-gvisor-cgroup-watchdog-armed-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let escaped = std::env::temp_dir().join(format!(
            "myelin-gvisor-cgroup-watchdog-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let permit = LaunchPermit::retained(|| Ok(crate::LaunchOwnership::immediate()));
        let mut command =
            SandboxCommand::new("/bin/sh", Some(permit), Some(Duration::from_secs(2))).unwrap();
        command
            .command_mut()
            .arg("-c")
            .arg(
                "setsid /bin/sh -c 'printf \"%s\" \"$$\" > \"$1\"; \
                 sleep 0.35; printf escaped > \"$2\"; sleep 5' \
                 myelin-detached \"$1\" \"$2\" & sleep 5",
            )
            .arg("myelin-cgroup-watchdog")
            .arg(&armed)
            .arg(&escaped)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cgroup
            .place_child(command.command_mut())
            .expect("place launch guard in the real gVisor cgroup");
        command.kill_cgroup_on_liveness_loss(
            cgroup
                .kill_file()
                .expect("open the real whole-cgroup kill switch"),
        );
        let mut child = command.spawn().expect("release detached-descendant probe");
        let runtime_group = child.id() as i32;
        let deadline = Instant::now() + Duration::from_secs(1);
        while !armed.exists() {
            assert!(
                Instant::now() < deadline,
                "detached cgroup descendant did not publish readiness"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let detached_pid: i32 = std::fs::read_to_string(&armed)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let detached_group = unsafe { libc::getpgid(detached_pid) };
        assert!(detached_group > 0);
        assert_ne!(
            detached_group, runtime_group,
            "the readiness process must genuinely be outside the runtime process group"
        );
        let membership = std::fs::read_to_string(format!("/proc/{detached_pid}/cgroup")).unwrap();
        let cgroup_name = cgroup.dir.file_name().unwrap().to_string_lossy();
        assert!(
            membership.contains(cgroup_name.as_ref()),
            "the detached descendant must remain in the exact gVisor memory cgroup"
        );
        child.kill_and_wait();
        std::thread::sleep(Duration::from_millis(450));
        assert!(
            !escaped.exists(),
            "a sentry/gofer-shaped descendant outside the runtime process group survived cgroup.kill"
        );
        let _ = std::fs::remove_file(armed);
        let _ = std::fs::remove_file(escaped);
    }
}

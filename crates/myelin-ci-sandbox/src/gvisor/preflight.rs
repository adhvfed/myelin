//! `runsc` binary resolution + the boot preflight that refuses activation unless the runtime,
//! the immutable base rootfs, and the delegated memory-cgroup boundary are all usable.

use super::*;
use crate::redaction::RedactionPlan;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Env var naming the `runsc` binary; defaults to `runsc` on `PATH`.
pub const ENV_RUNSC_BIN: &str = "MYELIN_RUNSC_BIN";

/// The resolved, CANONICALIZED, ABSOLUTE `runsc` binary path — computed ONCE and cached (Sol's
/// review, round 2: re-reading the env var on every launch means a boot preflight validating one
/// binary and a later launch resolving a DIFFERENT one, if the environment changed mid-process,
/// could silently diverge — caching makes "the boot-validated binary is the one every launch uses"
/// an actual invariant, not merely usually true). Round 3: [`preflight_gvisor_runner_host`], if
/// called, populates this SAME cell with its own already-canonicalized, already-probed path. Round
/// 4 correction: a `set()` conflict is NOT a harmless no-op — [`preflight_gvisor_runner_host`]
/// treats it as a hard preflight FAILURE (something else already cached a DIFFERENT path before
/// preflight ran), since silently keeping the earlier, unvalidated value would let preflight report
/// success for a binary no launch actually uses. A test or tool that calls
/// [`run_and_capture`]/[`delete_container`] directly, without ever calling preflight, gets the lazy
/// fallback below instead (best-effort, matching this function's pre-round-3 behavior).
static RESOLVED_RUNSC_BIN: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn resolved_runsc_bin_path() -> &'static Path {
    RESOLVED_RUNSC_BIN.get_or_init(|| {
        let configured = std::env::var(ENV_RUNSC_BIN).unwrap_or_else(|_| "runsc".to_string());
        let candidate = if configured.contains('/') {
            PathBuf::from(&configured)
        } else {
            std::env::var("PATH")
                .ok()
                .and_then(|paths| {
                    paths
                        .split(':')
                        .map(|dir| Path::new(dir).join(&configured))
                        .find(|candidate| candidate.exists())
                })
                .unwrap_or_else(|| PathBuf::from(&configured))
        };
        // Canonicalize when possible (resolves symlinks, makes the cached value absolute) —
        // falls back to the unresolved candidate when canonicalization fails (e.g. a test
        // environment with no `runsc` installed at all); resolution failure surfaces naturally as
        // a spawn error at first actual use, exactly as before this round's hardening.
        candidate.canonicalize().unwrap_or(candidate)
    })
}

/// Sol's review, round 4: returns the resolved `&Path` directly (was `&str` via `.to_str().
/// unwrap_or("runsc")`) — falling back to the unrelated literal `"runsc"` for a non-UTF-8 resolved
/// path would have silently swapped in a DIFFERENT, unvalidated binary resolution. `Command::new`/
/// `SandboxCommand::new` both accept `impl AsRef<OsStr>`, so callers pass this straight through.
pub(super) fn runsc_bin() -> &'static Path {
    resolved_runsc_bin_path()
}

/// How a [`probe_runsc_version`] boot preflight failed; the caller owns the operator-facing wording.
#[derive(Debug, PartialEq, Eq)]
pub enum RunscProbeError {
    /// The candidate carries metadata that would grant authority at `execve` time.
    UnsafeBinary(String),
    /// The binary could not be spawned for its `--version` probe.
    CouldNotExecute,
    /// The binary ran but did not identify itself as `runsc`.
    NotRunsc,
}

/// Boot-preflight probe: verify `path` identifies itself as `runsc` (via `--version`). Lives in
/// this crate — not the caller's — because the sandbox seam is the one sanctioned host-exec site
/// (no-host-exec, X-6/AG-2); serving binaries that require the sandbox at boot call this instead
/// of spawning the probe themselves.
pub fn probe_runsc_version(path: &Path) -> Result<(), RunscProbeError> {
    probe_runsc_version_given(path, reject_security_capability_xattr)
}

fn probe_runsc_version_given(
    path: &Path,
    reject_file_capabilities: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), RunscProbeError> {
    // File capabilities participate in execve's authority calculation but are not covered by the
    // binary's content digest. This check belongs directly at the public probe/exec seam so the
    // always-run rootless preflight cannot bypass the explicit-userns hardening helper.
    reject_file_capabilities(path).map_err(RunscProbeError::UnsafeBinary)?;
    let output = Command::new(path)
        .arg("--version")
        .output()
        .map_err(|_| RunscProbeError::CouldNotExecute)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !stdout.starts_with("runsc version ") {
        return Err(RunscProbeError::NotRunsc);
    }
    Ok(())
}

/// Boot preflight for a production gVisor runner host. This is deliberately stronger than waiting
/// for the first claimed job: activation must refuse before intake unless the exact runtime,
/// immutable base rootfs, and delegated memory-cgroup boundary are all usable.
pub fn preflight_gvisor_runner_host(runsc: &Path, rootfs: &Path) -> Result<(), String> {
    if !runsc.is_absolute() {
        return Err("MYELIN_RUNSC_BIN must be an absolute path".into());
    }
    let runsc = runsc
        .canonicalize()
        .map_err(|error| format!("MYELIN_RUNSC_BIN must name an existing executable: {error}"))?;
    if !is_executable_file(&runsc) {
        return Err("MYELIN_RUNSC_BIN must name an executable file".into());
    }
    probe_runsc_version(&runsc).map_err(|error| match error {
        RunscProbeError::UnsafeBinary(reason) => {
            format!("MYELIN_RUNSC_BIN failed executable metadata validation: {reason}")
        }
        RunscProbeError::CouldNotExecute => {
            "MYELIN_RUNSC_BIN could not execute its version probe".to_string()
        }
        RunscProbeError::NotRunsc => {
            "MYELIN_RUNSC_BIN did not identify itself as runsc".to_string()
        }
    })?;
    // Feed THIS canonicalized, probed path into the SAME cell `runsc_bin()` reads, so every
    // subsequent launch uses the exact binary this preflight just validated (Sol's review, round
    // 3) rather than a separately (if identically) re-derived path. Round 4: a `set()` failure
    // (something already cached a DIFFERENT value before this preflight ran) is now a PREFLIGHT
    // FAILURE, not a silently ignored no-op — otherwise preflight could report success having
    // validated binary B while every launch keeps using a stale, previously-cached binary A, with
    // nothing surfacing the divergence.
    if RESOLVED_RUNSC_BIN.set(runsc.clone()).is_err() {
        let already_cached = RESOLVED_RUNSC_BIN
            .get()
            .expect("set() just failed, so the cell must already be initialized");
        if already_cached != &runsc {
            return Err(format!(
                "MYELIN_RUNSC_BIN preflight validated {runsc:?}, but {already_cached:?} was \
                 already cached by an earlier resolution — refusing rather than leaving launches \
                 on a stale, unvalidated binary"
            ));
        }
    }

    if !rootfs.is_absolute() {
        return Err("MYELIN_GVISOR_ROOTFS must be an absolute path".into());
    }
    let rootfs = rootfs.canonicalize().map_err(|error| {
        format!("MYELIN_GVISOR_ROOTFS must name an existing directory: {error}")
    })?;
    if rootfs.parent().is_none() || !rootfs.is_dir() {
        return Err("MYELIN_GVISOR_ROOTFS must resolve to a non-root directory".into());
    }
    for relative in ["bin/sh", "bin/false"] {
        let executable = rootfs.join(relative);
        if !is_executable_file(&executable) {
            return Err(format!(
                "MYELIN_GVISOR_ROOTFS must contain executable {}",
                executable.display()
            ));
        }
    }

    let config = OciConfig {
        args: vec!["/bin/false".into()],
        root_readonly: true,
        drop_all_caps: true,
        no_new_privileges: true,
        seccomp: true,
        has_network: false,
        pids_max: 128,
        mem_bytes: 256 * 1024 * 1024,
        tmpfs_bytes: 1024 * 1024 * 1024,
        extra_env: Vec::new(),
        layout: OciExecutionLayout::Rootless,
    };
    // CT-007 slice 3, piece 7b (Sol's round-2 review, blocker 2): the single prepared mode this
    // preflight both executes under AND finalizes under — validated against `config` BEFORE
    // staging anything.
    let prepared_mode = PreparedRuntimeMode::Rootless;
    let mode = require_oci_layout_matches_prepared_mode(&config, &prepared_mode)
        .map_err(|error| format!("CI runner sandbox host preflight failed: {error}"))?;
    let mut bundle = stage_production_bundle(&config, &rootfs)
        .map_err(|error| format!("CI runner sandbox host preflight failed: {error}"))?;
    let container_id = format!(
        "myelin-preflight-{}-{}",
        std::process::id(),
        unique_suffix()
    );
    // CT-007 slice 3, piece 7b: same cgroup hoist as the production run paths — this preflight is
    // real production code (it gates host activation), so it gets the same checked teardown rather
    // than a best-effort `delete_container`/`Drop`-only cleanup.
    let cgroup = match MemoryCgroup::create(config.mem_bytes, 1000) {
        Ok(cgroup) => cgroup,
        Err(e) => {
            let cleanup = bundle.cleanup();
            return Err(match cleanup {
                Ok(()) => format!(
                    "CI runner sandbox host preflight failed: establish memory cgroup: {e}"
                ),
                Err(cleanup) => format!(
                    "CI runner sandbox host preflight failed: establish memory cgroup: {e}; {cleanup}"
                ),
            });
        }
    };
    let (result, child_retirement) = run_and_capture(
        &runsc,
        &bundle,
        &container_id,
        Duration::from_secs(5),
        config.mem_bytes,
        RunCaptureOptions {
            stdin: None,
            stdout_mode: StdoutMode::CappedHead,
            cancellation: &NEVER_CANCELLED,
            redaction: RedactionPlan::none(),
            output: None,
        },
        None,
        mode,
        &cgroup,
    );
    let finalize_result = finalize_runtime(
        &runsc,
        &container_id,
        &prepared_mode,
        cgroup,
        RUNTIME_QUIESCE_TIMEOUT,
        child_retirement,
    );
    let cleanup_result = bundle.cleanup();
    let outcome = preflight_capture_and_teardown_result(result, finalize_result)
        .map_err(|error| format!("CI runner sandbox host preflight failed: {error}"));
    let outcome = match (outcome, cleanup_result) {
        (Ok(outcome), Ok(())) => outcome,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(cleanup)) => {
            return Err(format!(
                "CI runner sandbox host preflight failed: {cleanup}"
            ));
        }
        (Err(error), Err(cleanup)) => return Err(format!("{error}; {cleanup}")),
    };
    if outcome.timed_out || outcome.exit != Some(1) {
        let stderr = String::from_utf8_lossy(&outcome.stderr);
        let stderr: String = stderr.chars().take(512).collect();
        return Err(format!(
            "CI runner sandbox host preflight `/bin/false` returned {:?}: {stderr}",
            outcome.exit,
        ));
    }
    Ok(())
}

/// The compound-failure decision behind [`preflight_gvisor_runner_host`] (Sol's round-2 review,
/// blocker 4), pulled out into its own pure function so it can be unit-tested with fabricated
/// values — a bare `?` on `result` before inspecting `finalize_result` would silently discard a
/// teardown failure whenever BOTH failed. Mirrors the same "augment, never replace" rule
/// [`augment_run_failure_with_teardown`] applies to the production path's `RunFailure`.
fn preflight_capture_and_teardown_result(
    result: Result<RunscOutcome, RunFailure>,
    finalize_result: Result<RuntimeQuiescenceEvidence, RuntimeTeardownError>,
) -> Result<RunscOutcome, String> {
    match (result, finalize_result) {
        (Ok(outcome), Ok(_evidence)) => Ok(outcome),
        (Ok(_outcome), Err(teardown)) => Err(format!("runtime teardown check failed: {teardown}")),
        (Err(capture_error), Ok(_evidence)) => Err(capture_error.to_string()),
        (Err(capture_error), Err(teardown)) => Err(format!(
            "{capture_error} AND runtime teardown check also failed: {teardown}"
        )),
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    use crate::gvisor::test_fixtures::*;
    use std::time::Duration;

    #[test]
    fn runner_host_preflight_refuses_a_non_absolute_runtime_before_intake() {
        let error = preflight_gvisor_runner_host(Path::new("runsc"), Path::new("/unused-rootfs"))
            .expect_err("a PATH-relative runtime is not stable production authority");
        assert!(error.contains("MYELIN_RUNSC_BIN must be an absolute path"));
    }

    #[test]
    fn rootless_version_probe_rejects_an_unexpected_file_capability_before_exec() {
        let result = probe_runsc_version_given(Path::new("/definitely/not/executable"), |_| {
            Err("unexpected security.capability xattr".to_string())
        });
        assert_eq!(
            result,
            Err(RunscProbeError::UnsafeBinary(
                "unexpected security.capability xattr".to_string()
            )),
            "the rootless startup probe must reject metadata before attempting --version"
        );
    }

    #[test]
    fn preflight_capture_and_teardown_result_passes_through_a_clean_success() {
        let evidence = RuntimeQuiescenceEvidence {
            container_id: "c".to_string(),
            namespace: RuntimeNamespaceQuiescence::Rootless,
            cgroup: CgroupQuiescenceEvidence::assert_for_tests((1, 2)),
        };
        let result = preflight_capture_and_teardown_result(Ok(outcome(b"", b"")), Ok(evidence));
        assert!(result.is_ok(), "expected Ok, got Err");
    }

    #[test]
    fn preflight_capture_and_teardown_result_surfaces_a_teardown_only_failure() {
        let teardown = RuntimeTeardownError {
            issues: vec![RuntimeTeardownIssue::Cgroup(
                CgroupQuiescenceError::StillPopulated {
                    waited: Duration::from_secs(2),
                },
            )],
        };
        let result = preflight_capture_and_teardown_result(Ok(outcome(b"", b"")), Err(teardown));
        let Err(message) = result else {
            panic!("a teardown-only failure must still refuse");
        };
        assert!(
            message.contains("runtime teardown check failed"),
            "{message}"
        );
    }

    #[test]
    fn preflight_capture_and_teardown_result_surfaces_a_capture_only_failure() {
        let evidence = RuntimeQuiescenceEvidence {
            container_id: "c".to_string(),
            namespace: RuntimeNamespaceQuiescence::Rootless,
            cgroup: CgroupQuiescenceEvidence::assert_for_tests((1, 2)),
        };
        let capture_failure = RunFailure::uncommitted("spawn runsc: boom");
        let result = preflight_capture_and_teardown_result(Err(capture_failure), Ok(evidence));
        let Err(message) = result else {
            panic!("a capture-only failure must still refuse");
        };
        assert!(message.contains("boom"), "{message}");
    }

    /// Sol's round-2 review, blocker 4: the previous implementation applied `?` to the capture
    /// result BEFORE ever inspecting the teardown result — when BOTH failed, the teardown
    /// diagnostic silently disappeared. Proves both messages survive when both fail.
    #[test]
    fn preflight_capture_and_teardown_result_reports_both_failures_when_both_fail() {
        let capture_failure = RunFailure::uncommitted("spawn runsc: boom");
        let teardown = RuntimeTeardownError {
            issues: vec![RuntimeTeardownIssue::Cgroup(
                CgroupQuiescenceError::StillPopulated {
                    waited: Duration::from_secs(2),
                },
            )],
        };
        let result = preflight_capture_and_teardown_result(Err(capture_failure), Err(teardown));
        let Err(message) = result else {
            panic!("a compound failure must still refuse");
        };
        assert!(message.contains("boom"), "{message}");
        assert!(
            message.contains("runtime teardown check also failed"),
            "{message}"
        );
    }
}

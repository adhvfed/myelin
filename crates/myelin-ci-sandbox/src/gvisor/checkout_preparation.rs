use super::*;
use crate::hardening::HardeningProfile;
use crate::redaction::RedactionPlan;
use crate::runner::{
    PreparationAttemptDisposition, PreparationPhase, PreparationTerminalDisposition,
};
use crate::user_namespace::{
    CheckoutPreparationSession, CheckoutSessionCleanup, PreparationQuiescenceProof,
    UserNamespaceBindError, UserNamespaceLease,
};
use crate::workspace_intent::ExpectedGitCommitId;
use crate::workspace_manager::{ManagedWorkspace, WorkspaceManager};
use crate::{
    CheckoutAuthorizationScope, EgressPolicy, LaunchPermit, PhaseAuthorization, ResourceLimits,
    ResourceUsage, RunTokenCredential, SandboxOutputSink,
};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::io;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct CheckoutPreparationSpec {
    pub(super) expected_commit: ExpectedGitCommitId,
    pack: PrefetchedCheckoutPack,
    limits: ResourceLimits,
}

impl CheckoutPreparationSpec {
    #[allow(dead_code)]
    pub(crate) fn new(
        expected_commit: ExpectedGitCommitId,
        pack: PrefetchedCheckoutPack,
        limits: ResourceLimits,
    ) -> Result<Self, String> {
        crate::validate_execution_limits(&limits)?;
        Ok(CheckoutPreparationSpec {
            expected_commit,
            pack,
            limits,
        })
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct PreparedCheckoutEvidence {
    commit_hex: String,
    tree_oid: String,
    cargo_lock_sha256_hex: String,
    preparation_usage: ResourceUsage,
}

impl PreparedCheckoutEvidence {
    #[allow(dead_code)]
    pub(crate) fn commit_hex(&self) -> &str {
        &self.commit_hex
    }

    #[allow(dead_code)]
    pub(crate) fn tree_oid(&self) -> &str {
        &self.tree_oid
    }

    #[allow(dead_code)]
    pub(crate) fn cargo_lock_sha256_hex(&self) -> &str {
        &self.cargo_lock_sha256_hex
    }

    #[allow(dead_code)]
    pub(crate) fn preparation_usage(&self) -> ResourceUsage {
        self.preparation_usage
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn for_tests(preparation_usage: ResourceUsage) -> Self {
        PreparedCheckoutEvidence {
            commit_hex: "a".repeat(40),
            tree_oid: "b".repeat(40),
            cargo_lock_sha256_hex: "c".repeat(64),
            preparation_usage,
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum CheckoutPreparationError {
    Refused(String),
    Unreleasable {
        message: String,
        usage: Option<ResourceUsage>,
    },
    TeardownUnproven {
        message: String,
        usage: ResourceUsage,
    },
    RejectedAfterQuiescence {
        message: String,
        usage: ResourceUsage,
        disposition: PreparationAttemptDisposition,
    },
}

impl CheckoutPreparationError {
    #[allow(dead_code)]
    pub(crate) fn attempt_disposition(&self) -> PreparationAttemptDisposition {
        match self {
            Self::Refused(_) => PreparationAttemptDisposition::RefusedBeforeExecution {
                phase: PreparationPhase::CheckoutMaterialization,
            },
            Self::Unreleasable { .. } => PreparationAttemptDisposition::ReconciliationRequired {
                phase: PreparationPhase::CheckoutMaterialization,
                teardown_unproven: false,
                usage_unrepresentable: false,
                quarantine_required: true,
            },
            Self::TeardownUnproven { .. } => {
                PreparationAttemptDisposition::ReconciliationRequired {
                    phase: PreparationPhase::CheckoutMaterialization,
                    teardown_unproven: true,
                    usage_unrepresentable: false,
                    quarantine_required: true,
                }
            }
            Self::RejectedAfterQuiescence { disposition, .. } => *disposition,
        }
    }
}

fn checkout_materialization_terminal_failed(
    message: String,
    usage: ResourceUsage,
) -> CheckoutPreparationError {
    CheckoutPreparationError::RejectedAfterQuiescence {
        message,
        usage,
        disposition: PreparationAttemptDisposition::Terminal(
            PreparationTerminalDisposition::Failed {
                phase: PreparationPhase::CheckoutMaterialization,
            },
        ),
    }
}

pub(super) fn checkout_materialization_timed_out(
    message: String,
    usage: ResourceUsage,
) -> CheckoutPreparationError {
    CheckoutPreparationError::RejectedAfterQuiescence {
        message,
        usage,
        disposition: PreparationAttemptDisposition::Terminal(
            PreparationTerminalDisposition::TimedOut {
                phase: PreparationPhase::CheckoutMaterialization,
            },
        ),
    }
}

fn checkout_materialization_retryable(
    message: String,
    usage: ResourceUsage,
) -> CheckoutPreparationError {
    CheckoutPreparationError::RejectedAfterQuiescence {
        message,
        usage,
        disposition: PreparationAttemptDisposition::RetryableInfrastructure {
            phase: PreparationPhase::CheckoutMaterialization,
        },
    }
}

pub(super) fn map_checkout_materialization_run_failure(
    failure: RunFailure,
) -> CheckoutPreparationError {
    match failure {
        failure @ RunFailure::CommitOutcomeUnknown { .. } => {
            CheckoutPreparationError::RejectedAfterQuiescence {
                message: format!(
                    "internal invariant violated (an immediate launch permit's commit closure \
                     cannot fail, so this should be unreachable): the run itself failed: {failure}"
                ),
                usage: ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0,
                },
                disposition: PreparationAttemptDisposition::ReconciliationRequired {
                    phase: PreparationPhase::CheckoutMaterialization,
                    teardown_unproven: false,
                    usage_unrepresentable: false,
                    quarantine_required: false,
                },
            }
        }
        failure @ (RunFailure::Uncommitted { .. } | RunFailure::CommittedButNotExecuted { .. }) => {
            checkout_materialization_retryable(
                format!("the run itself failed: {failure}"),
                ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0,
                },
            )
        }
        failure @ RunFailure::Executed { usage, .. } => {
            checkout_materialization_retryable(format!("the run itself failed: {failure}"), usage)
        }
    }
}

impl std::fmt::Display for CheckoutPreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckoutPreparationError::Refused(m) => write!(f, "checkout preparation refused: {m}"),
            CheckoutPreparationError::Unreleasable { message, .. } => {
                write!(f, "checkout preparation lease/session poisoned: {message}")
            }
            CheckoutPreparationError::TeardownUnproven { message, .. } => {
                write!(f, "checkout preparation teardown unproven: {message}")
            }
            CheckoutPreparationError::RejectedAfterQuiescence { message, .. } => {
                write!(f, "checkout rejected after proven teardown: {message}")
            }
        }
    }
}

impl std::error::Error for CheckoutPreparationError {}

#[allow(dead_code)]
const CHECKOUT_PREPARATION_SCRIPT: &str = "set -eu
umask 077
export HOME=/tmp
export GIT_CONFIG_NOSYSTEM=1
export GIT_EXEC_PATH=/usr/lib/git-core
oid=\"$1\"
format=\"$2\"
shallow=\"$3\"
cd /workspace
if [ -n \"$(ls -A .)\" ]; then
  echo 'workspace is not empty before checkout' 1>&2
  exit 1
fi
git -c core.hooksPath=/dev/null init -q --template= \"--object-format=$format\"
if [ \"$shallow\" = \"1\" ]; then
  printf '%s\\n' \"$oid\" > .git/shallow
fi
git index-pack --stdin --strict 1>&2
test \"$(git rev-parse --verify \"$oid^{commit}\")\" = \"$oid\"
tree_listing=\"$(git ls-tree -r \"$oid\")\" || { echo 'git ls-tree failed' 1>&2; exit 1; }
if printf '%s\\n' \"$tree_listing\" | grep -q '^160000 commit'; then
  gitlink_grep_status=0
else
  gitlink_grep_status=$?
fi
if [ \"$gitlink_grep_status\" -eq 0 ]; then
  echo 'gitlinks (submodules) are not supported by this checkout transport' 1>&2
  exit 1
elif [ \"$gitlink_grep_status\" -ne 1 ]; then
  echo 'gitlink check itself failed (unexpected grep exit status)' 1>&2
  exit 1
fi
git -c core.hooksPath=/dev/null checkout -q --detach \"$oid\"
test \"$(git rev-parse --verify HEAD)\" = \"$oid\"
git update-index -q --refresh
git diff-index --quiet \"$oid\" --
tree=\"$(git rev-parse --verify \"$oid^{tree}\")\"
printf '%s %s\\n' \"$oid\" \"$tree\"
";

#[cfg(test)]
const GITLINK_CHECK_SNIPPET_FOR_TESTS: &str = "set -eu
oid=\"$1\"
tree_listing=\"$(git ls-tree -r \"$oid\")\" || { echo 'git ls-tree failed' 1>&2; exit 1; }
if printf '%s\\n' \"$tree_listing\" | grep -q '^160000 commit'; then
  gitlink_grep_status=0
else
  gitlink_grep_status=$?
fi
if [ \"$gitlink_grep_status\" -eq 0 ]; then
  echo 'gitlinks (submodules) are not supported by this checkout transport' 1>&2
  exit 1
elif [ \"$gitlink_grep_status\" -ne 1 ]; then
  echo 'gitlink check itself failed (unexpected grep exit status)' 1>&2
  exit 1
fi
echo ok
";

#[allow(dead_code)]
fn parse_checkout_confirmation_line(
    stdout: &[u8],
    expected: &ExpectedGitCommitId,
) -> Result<String, String> {
    let text = std::str::from_utf8(stdout)
        .map_err(|_| "checkout confirmation output is not UTF-8".to_string())?;
    let line = text.strip_suffix('\n').unwrap_or(text);
    let mut parts = line.split(' ');
    let commit = parts
        .next()
        .ok_or_else(|| "checkout confirmation output is empty".to_string())?;
    let tree = parts
        .next()
        .ok_or_else(|| "checkout confirmation output is missing the tree oid".to_string())?;
    if parts.next().is_some() {
        return Err(format!(
            "checkout confirmation output has unexpected extra fields: {line:?}"
        ));
    }
    if commit != expected.as_str() {
        return Err(format!(
            "checkout confirmation reports commit {commit:?}, expected {:?}",
            expected.as_str()
        ));
    }
    let width = expected.format().hex_width();
    if tree.len() != width
        || !tree
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(format!(
            "checkout confirmation tree oid {tree:?} is not valid {width}-char hex"
        ));
    }
    Ok(tree.to_string())
}

#[allow(dead_code)]
fn open_regular_file_no_follow(dir_fd: RawFd, name: &CString) -> io::Result<std::fs::File> {
    let fd = unsafe {
        libc::openat(
            dir_fd,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a regular file",
        ));
    }
    Ok(file)
}

#[allow(dead_code)]
fn verify_workspace_head_no_follow(
    workspace_host_path: &Path,
    expected: &ExpectedGitCommitId,
) -> Result<(), String> {
    let path_c = CString::new(workspace_host_path.as_os_str().as_encoded_bytes())
        .map_err(|e| format!("workspace path contains an interior NUL: {e}"))?;
    let workspace_fd = unsafe {
        libc::open(
            path_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if workspace_fd < 0 {
        return Err(format!(
            "open workspace directory: {}",
            io::Error::last_os_error()
        ));
    }
    let workspace_fd = unsafe { OwnedFd::from_raw_fd(workspace_fd) };
    let git_name = CString::new(".git").expect("no interior NUL");
    let git_fd = crate::dirlock::open_dir_component_no_follow(workspace_fd.as_raw_fd(), &git_name)
        .map_err(|e| format!(".git is not a real directory (or is a symlink): {e}"))?;
    let head_name = CString::new("HEAD").expect("no interior NUL");
    let mut head_file = open_regular_file_no_follow(git_fd.as_raw_fd(), &head_name)
        .map_err(|e| format!(".git/HEAD is not a real regular file (or is a symlink): {e}"))?;
    let mut buf = Vec::new();
    std::io::Read::by_ref(&mut head_file)
        .take(129)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read .git/HEAD: {e}"))?;
    if buf.len() > 128 {
        return Err(format!(
            ".git/HEAD is implausibly large (>= {} bytes)",
            buf.len()
        ));
    }
    let content =
        String::from_utf8(buf).map_err(|_| ".git/HEAD content is not UTF-8".to_string())?;
    let expected_line = format!("{}\n", expected.as_str());
    if content != expected_line {
        return Err(format!(
            ".git/HEAD content {content:?} does not exactly match the expected detached commit \
             {expected_line:?}"
        ));
    }
    Ok(())
}

const CARGO_LOCK_HASH_BOUND: u64 = 16 * 1024 * 1024;

fn hash_workspace_cargo_lock_no_follow(workspace_host_path: &Path) -> Result<String, String> {
    let path_c = CString::new(workspace_host_path.as_os_str().as_encoded_bytes())
        .map_err(|e| format!("workspace path contains an interior NUL: {e}"))?;
    let workspace_fd = unsafe {
        libc::open(
            path_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if workspace_fd < 0 {
        return Err(format!(
            "open workspace directory: {}",
            io::Error::last_os_error()
        ));
    }
    let workspace_fd = unsafe { OwnedFd::from_raw_fd(workspace_fd) };
    let name = CString::new("Cargo.lock").expect("no interior NUL");
    let mut file = open_regular_file_no_follow(workspace_fd.as_raw_fd(), &name).map_err(|e| {
        format!("Cargo.lock is not present as a real regular file (or is a symlink): {e}")
    })?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut chunk)
            .map_err(|e| format!("read Cargo.lock: {e}"))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > CARGO_LOCK_HASH_BOUND {
            return Err(format!(
                "Cargo.lock exceeds the {CARGO_LOCK_HASH_BOUND}-byte bound"
            ));
        }
        hasher.update(&chunk[..n]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}

struct ScopedDacReadSearch {
    active: bool,
}

impl ScopedDacReadSearch {
    fn enter() -> Result<Self, String> {
        let mut capabilities = current_thread_capabilities()
            .map_err(|error| format!("read verifier-thread capabilities: {error}"))?;
        if !capability_is_permitted(&capabilities, CAP_DAC_READ_SEARCH_NUMBER) {
            return Err(
                "CAP_DAC_READ_SEARCH is absent from the verifier thread's permitted set"
                    .to_string(),
            );
        }
        if capability_is_effective(&capabilities, CAP_DAC_READ_SEARCH_NUMBER) {
            return Err(
                "CAP_DAC_READ_SEARCH was already effective before the bounded host-verification \
                 scope; startup must leave it permitted-only"
                    .to_string(),
            );
        }
        set_capability_effective(&mut capabilities, CAP_DAC_READ_SEARCH_NUMBER, true);
        set_current_thread_capabilities(&capabilities)
            .map_err(|error| format!("enable scoped CAP_DAC_READ_SEARCH: {error}"))?;
        Ok(Self { active: true })
    }

    fn clear_current_thread() -> Result<(), String> {
        let mut capabilities = current_thread_capabilities()
            .map_err(|error| format!("read verifier-thread capabilities for restore: {error}"))?;
        set_capability_effective(&mut capabilities, CAP_DAC_READ_SEARCH_NUMBER, false);
        set_current_thread_capabilities(&capabilities)
            .map_err(|error| format!("clear scoped CAP_DAC_READ_SEARCH: {error}"))?;
        let verified = current_thread_capabilities()
            .map_err(|error| format!("re-read restored verifier-thread capabilities: {error}"))?;
        if capability_is_effective(&verified, CAP_DAC_READ_SEARCH_NUMBER) {
            return Err(
                "CAP_DAC_READ_SEARCH remained effective after the verifier scope".to_string(),
            );
        }
        Ok(())
    }

    fn finish(mut self) -> Result<(), String> {
        Self::clear_current_thread()?;
        self.active = false;
        Ok(())
    }
}

impl Drop for ScopedDacReadSearch {
    fn drop(&mut self) {
        if self.active && Self::clear_current_thread().is_err() {
            std::process::abort();
        }
    }
}

fn verify_materialized_checkout_no_follow(
    workspace_host_path: &Path,
    expected: &ExpectedGitCommitId,
) -> Result<String, String> {
    let capabilities = current_thread_capabilities()
        .map_err(|error| format!("read host-verifier capability state: {error}"))?;
    let guard = if capability_is_permitted(&capabilities, CAP_DAC_READ_SEARCH_NUMBER) {
        Some(ScopedDacReadSearch::enter()?)
    } else {
        None
    };

    let result = verify_workspace_head_no_follow(workspace_host_path, expected)
        .map_err(|reason| format!("host-side HEAD re-verification disagreed: {reason}"))
        .and_then(|()| {
            hash_workspace_cargo_lock_no_follow(workspace_host_path)
                .map_err(|reason| format!("could not hash the materialized Cargo.lock: {reason}"))
        });

    if let Some(guard) = guard {
        guard.finish()?;
    }
    result
}

#[allow(dead_code)]
fn usage_from_runsc_outcome(mem_bytes: u64, o: &RunscOutcome) -> ResourceUsage {
    let wall_secs_ceil = o.wall.as_secs() + u64::from(o.wall.subsec_nanos() > 0);
    let cpu_seconds = o.cpu_seconds.filter(|c| *c > 0).unwrap_or(wall_secs_ceil);
    ResourceUsage {
        cpu_seconds,
        mem_byte_seconds: mem_bytes.saturating_mul(wall_secs_ceil),
    }
}

#[allow(dead_code)]
pub(crate) fn run_checkout_preparation(
    lease: &mut UserNamespaceLease,
    session: &mut CheckoutPreparationSession,
    workspace: &ManagedWorkspace,
    spec: CheckoutPreparationSpec,
) -> Result<PreparedCheckoutEvidence, CheckoutPreparationError> {
    run_checkout_preparation_inner(
        lease,
        session,
        workspace,
        spec,
        LaunchPermit::immediate(),
        &NEVER_CANCELLED,
        None,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(super) enum CheckoutCleanupPlan {
    DeleteWorkspaceThenReleaseUnused,
    DeleteWorkspaceThenReleasePrepared,
    QuarantineBoth,
    AbandonBoth,
}

#[allow(dead_code)]
pub(super) fn checkout_cleanup_plan(disposition: CheckoutSessionCleanup) -> CheckoutCleanupPlan {
    match disposition {
        CheckoutSessionCleanup::NeverBound => CheckoutCleanupPlan::DeleteWorkspaceThenReleaseUnused,
        CheckoutSessionCleanup::Prepared => CheckoutCleanupPlan::DeleteWorkspaceThenReleasePrepared,
        CheckoutSessionCleanup::TeardownUnproven | CheckoutSessionCleanup::Unreleasable => {
            CheckoutCleanupPlan::QuarantineBoth
        }
        CheckoutSessionCleanup::WorkloadBound => CheckoutCleanupPlan::AbandonBoth,
    }
}

#[allow(dead_code)]
pub(super) trait CheckoutCleanupExecutor {
    fn delete_workspace(&mut self) -> (bool, Vec<String>);
    fn release_unused(&mut self) -> Vec<String>;
    fn release_prepared(&mut self) -> Vec<String>;
    fn quarantine_workspace(&mut self);
    fn quarantine_lease(&mut self);
}

#[allow(dead_code)]
pub(super) fn execute_cleanup_plan(
    plan: CheckoutCleanupPlan,
    executor: &mut dyn CheckoutCleanupExecutor,
) -> Vec<String> {
    let mut diagnostics = Vec::new();
    match plan {
        CheckoutCleanupPlan::DeleteWorkspaceThenReleaseUnused => {
            let (proven, d) = executor.delete_workspace();
            diagnostics.extend(d);
            if proven {
                diagnostics.extend(executor.release_unused());
            } else {
                executor.quarantine_lease();
            }
        }
        CheckoutCleanupPlan::DeleteWorkspaceThenReleasePrepared => {
            let (proven, d) = executor.delete_workspace();
            diagnostics.extend(d);
            if proven {
                diagnostics.extend(executor.release_prepared());
            } else {
                executor.quarantine_lease();
            }
        }
        CheckoutCleanupPlan::QuarantineBoth => {
            executor.quarantine_workspace();
            executor.quarantine_lease();
            diagnostics.push(
                "checkout runtime disposed with an unproven teardown (or an already-abandoned \
                 lease): the workspace and lease are both quarantined, never released"
                    .to_string(),
            );
        }
        CheckoutCleanupPlan::AbandonBoth => {
            executor.quarantine_workspace();
            executor.quarantine_lease();
            diagnostics.push(
                "dispose_checkout_runtime reached a WorkloadBound capsule - this should be \
                 structurally impossible (a bound workload's workspace and lease are owned by the \
                 existing finalization/settlement path); both are abandoned (quarantined) rather \
                 than acted on"
                    .to_string(),
            );
        }
    }
    diagnostics
}

#[allow(dead_code)]
pub(super) struct RealCheckoutCleanupExecutor<'a> {
    pub(super) workspace: Option<ManagedWorkspace>,
    pub(super) lease: Option<UserNamespaceLease>,
    pub(super) checkout_session: Option<CheckoutPreparationSession>,
    pub(super) workspace_manager: &'a WorkspaceManager,
}

#[allow(dead_code)]
impl CheckoutCleanupExecutor for RealCheckoutCleanupExecutor<'_> {
    fn delete_workspace(&mut self) -> (bool, Vec<String>) {
        let workspace = self
            .workspace
            .take()
            .expect("delete_workspace invoked once, with the workspace still held");
        match classify_workspace_deletion(self.workspace_manager.delete_workspace(workspace)) {
            WorkspaceDeletionOutcome::ProvenAbsent { diagnostic } => {
                (true, diagnostic.into_iter().collect())
            }
            WorkspaceDeletionOutcome::NotProvenAbsent { diagnostic } => (false, vec![diagnostic]),
        }
    }

    fn release_unused(&mut self) -> Vec<String> {
        let lease = self
            .lease
            .take()
            .expect("release_unused invoked once, with the lease still held");
        match lease.release_unused() {
            Ok(()) => Vec::new(),
            Err(e) => vec![format!("releasing the unused userns lease failed: {e}")],
        }
    }

    fn release_prepared(&mut self) -> Vec<String> {
        let session = self
            .checkout_session
            .take()
            .expect("release_prepared invoked once, with the session still held");
        let lease = self
            .lease
            .take()
            .expect("release_prepared invoked once, with the lease still held");
        match session.release_prepared(lease) {
            Ok(()) => Vec::new(),
            Err(e) => vec![format!("releasing the prepared userns lease failed: {e}")],
        }
    }

    fn quarantine_workspace(&mut self) {
        drop(self.workspace.take());
    }

    fn quarantine_lease(&mut self) {
        drop(self.lease.take());
    }
}

#[allow(dead_code)]
pub(crate) enum RetainedWorkloadOutcome {
    Ran(Result<ContainerRun, RunFailure>),
    RunFailed {
        failure: RunFailure,
        disposal_diagnostics: Vec<String>,
    },
    PhaseAuthorityFailed {
        error: crate::checkout_orchestration::AttemptAuthorityError,
        disposal_diagnostics: Vec<String>,
    },
    LeaseLost {
        lost: crate::runner::PreparationLeaseLost,
        disposal_diagnostics: Vec<String>,
    },
    PermitRefused {
        message: String,
        disposal_diagnostics: Vec<String>,
    },
}

#[allow(dead_code)]
pub(super) fn resolve_checkout_preparation_permit(
    authorization: PhaseAuthorization,
    run_token: &RunTokenCredential,
    checkout_scope: &CheckoutAuthorizationScope,
    expected_commit: &ExpectedGitCommitId,
) -> Result<LaunchPermit, CheckoutPreparationError> {
    authorization
        .into_preparation_permit_for_scope(run_token, checkout_scope, expected_commit)
        .map_err(|error| CheckoutPreparationError::Refused(error.0))
}

#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_checkout_preparation_inner(
    lease: &mut UserNamespaceLease,
    session: &mut CheckoutPreparationSession,
    workspace: &ManagedWorkspace,
    spec: CheckoutPreparationSpec,
    launch_permit: LaunchPermit,
    cancellation: &AtomicBool,
    output: Option<Arc<dyn SandboxOutputSink>>,
) -> Result<PreparedCheckoutEvidence, CheckoutPreparationError> {
    let bin = runsc_bin();
    let root_abs = verified_gvisor_git_rootfs().map_err(CheckoutPreparationError::Refused)?;
    let userns = lease.config();
    let workspace_mount = OciWorkspaceMount::from_managed_workspace(workspace);

    let profile = HardeningProfile::for_execution(&spec.limits, &EgressPolicy::deny_all());
    profile
        .assert_enforced()
        .map_err(CheckoutPreparationError::Refused)?;
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        CHECKOUT_PREPARATION_SCRIPT.to_string(),
        "sh".to_string(),
        spec.expected_commit.as_str().to_string(),
        spec.expected_commit.format().init_token().to_string(),
        if spec.pack.shallow { "1" } else { "0" }.to_string(),
    ];
    let cfg = OciConfig::for_fixed_command(command, spec.limits.mem_bytes, &profile)
        .with_explicit_user_namespace_and_workspace(userns, workspace_mount, root_abs)
        .map_err(CheckoutPreparationError::Refused)?;

    let expected_root_identity = revalidated_explicit_userns_root_identity().map_err(|reason| {
        CheckoutPreparationError::Refused(format!(
            "runsc-root identity revalidation failed: {reason}"
        ))
    })?;
    let prepared_mode = PreparedRuntimeMode::ExplicitUserNamespace {
        config: userns,
        expected_root_identity,
    };
    let mode = require_oci_layout_matches_prepared_mode(&cfg, &prepared_mode)
        .map_err(CheckoutPreparationError::Refused)?;

    let bundle_dir = stage_config_only_bundle(&cfg, "checkout")
        .map_err(|e| CheckoutPreparationError::Refused(e.message))?;

    let cgroup = match MemoryCgroup::create(spec.limits.mem_bytes, spec.limits.cpu_millis) {
        Ok(cgroup) => cgroup,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&bundle_dir);
            return Err(CheckoutPreparationError::Refused(e));
        }
    };
    let container_id = format!("myelin-checkout-{}-{}", std::process::id(), unique_suffix());

    let current_root_identity = match revalidated_explicit_userns_root_identity() {
        Ok(identity) => identity,
        Err(reason) => {
            let _ = cgroup.quiesce(RUNTIME_QUIESCE_TIMEOUT);
            let _ = std::fs::remove_dir_all(&bundle_dir);
            return Err(CheckoutPreparationError::Refused(format!(
                "runsc-root identity revalidation failed before bind: {reason}"
            )));
        }
    };
    if current_root_identity != expected_root_identity {
        let _ = cgroup.quiesce(RUNTIME_QUIESCE_TIMEOUT);
        let _ = std::fs::remove_dir_all(&bundle_dir);
        return Err(CheckoutPreparationError::Refused(format!(
            "runsc-root identity drifted before bind (expected {expected_root_identity:?}, found \
             {current_root_identity:?})"
        )));
    }
    if let Err(bind_error) = session.bind_preparation(
        lease,
        container_id.clone(),
        current_root_identity,
        cgroup.identity(),
    ) {
        let _ = cgroup.quiesce(RUNTIME_QUIESCE_TIMEOUT);
        let _ = std::fs::remove_dir_all(&bundle_dir);
        return Err(match bind_error {
            UserNamespaceBindError::InvalidContainerId | UserNamespaceBindError::MarkerTooLarge => {
                CheckoutPreparationError::Refused(format!("bind_preparation: {bind_error}"))
            }
            UserNamespaceBindError::MarkerMismatch | UserNamespaceBindError::Poisoned => {
                CheckoutPreparationError::Unreleasable {
                    message: format!("bind_preparation: {bind_error}"),
                    usage: None,
                }
            }
        });
    }

    let timeout = Duration::from_secs(spec.limits.timeout_secs as u64);
    let (result, child_retirement) = run_and_capture(
        bin,
        &bundle_dir,
        &container_id,
        timeout,
        spec.limits.mem_bytes,
        RunCaptureOptions {
            stdin: Some(StdinSource::File(spec.pack.file)),
            stdout_mode: StdoutMode::CappedHead,
            cancellation,
            redaction: RedactionPlan::none(),
            output: output.map(|sink| StreamingOutput { sink }),
        },
        Some(launch_permit),
        mode,
        &cgroup,
    );
    let finalization = finalize_and_merge(
        result,
        bin,
        &container_id,
        &prepared_mode,
        cgroup,
        RUNTIME_QUIESCE_TIMEOUT,
        child_retirement,
    );
    let outcome = evaluate_checkout_finalization(
        finalization,
        lease,
        session,
        spec.limits.mem_bytes,
        &spec.expected_commit,
        workspace.host_path(),
    );
    let _ = std::fs::remove_dir_all(&bundle_dir);
    outcome
}

fn evaluate_checkout_finalization(
    finalization: RuntimeFinalization<Result<RunscOutcome, RunFailure>>,
    lease: &mut UserNamespaceLease,
    session: &mut CheckoutPreparationSession,
    mem_bytes: u64,
    expected_commit: &ExpectedGitCommitId,
    workspace_host_path: &Path,
) -> Result<PreparedCheckoutEvidence, CheckoutPreparationError> {
    fn usage_of(primary: &Result<RunscOutcome, RunFailure>, mem_bytes: u64) -> ResourceUsage {
        match primary {
            Ok(outcome) => usage_from_runsc_outcome(mem_bytes, outcome),
            Err(RunFailure::Executed { usage, .. }) => *usage,
            Err(_) => ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            },
        }
    }

    let (primary, evidence) = match finalization {
        RuntimeFinalization::Finalized(FinalizedRun { primary, evidence }) => (primary, evidence),
        RuntimeFinalization::Failed { primary, teardown } => {
            let usage = usage_of(&primary, mem_bytes);
            return Err(CheckoutPreparationError::TeardownUnproven {
                message: format!(
                    "runtime teardown could not be independently proven ({teardown}); primary \
                     run outcome: {}",
                    describe_run_primary(&primary)
                ),
                usage,
            });
        }
    };

    let proof = match PreparationQuiescenceProof::from_runtime_evidence(lease, &evidence) {
        Ok(proof) => proof,
        Err(reason) => {
            let usage = usage_of(&primary, mem_bytes);
            return Err(CheckoutPreparationError::TeardownUnproven {
                message: format!("could not mint a preparation quiescence proof: {reason}"),
                usage,
            });
        }
    };
    if let Err(confirm_error) = session.confirm_prepared(lease, proof) {
        let usage = usage_of(&primary, mem_bytes);
        return Err(CheckoutPreparationError::Unreleasable {
            message: format!("confirm_prepared: {confirm_error}"),
            usage: Some(usage),
        });
    }

    let outcome = match primary {
        Ok(outcome) => outcome,
        Err(failure) => return Err(map_checkout_materialization_run_failure(failure)),
    };
    let usage = usage_from_runsc_outcome(mem_bytes, &outcome);
    if outcome.timed_out {
        return Err(checkout_materialization_timed_out(
            "the checkout container timed out".to_string(),
            usage,
        ));
    }
    if outcome.stdout_truncated {
        return Err(checkout_materialization_terminal_failed(
            "the checkout confirmation output was truncated (exceeded its capture bound)"
                .to_string(),
            usage,
        ));
    }
    if let Some(stream_error) = &outcome.stream_error {
        return Err(checkout_materialization_retryable(
            format!("a stream/output error occurred during the checkout run: {stream_error}"),
            usage,
        ));
    }
    if outcome.exit != Some(0) {
        return Err(checkout_materialization_terminal_failed(
            format!(
                "the checkout script exited {:?} (stderr: {})",
                outcome.exit,
                String::from_utf8_lossy(&outcome.stderr)
            ),
            usage,
        ));
    }
    let tree_oid = match parse_checkout_confirmation_line(&outcome.stdout, expected_commit) {
        Ok(tree) => tree,
        Err(reason) => return Err(checkout_materialization_terminal_failed(reason, usage)),
    };
    let cargo_lock_sha256_hex =
        match verify_materialized_checkout_no_follow(workspace_host_path, expected_commit) {
            Ok(hex) => hex,
            Err(reason) => return Err(checkout_materialization_terminal_failed(reason, usage)),
        };

    Ok(PreparedCheckoutEvidence {
        commit_hex: expected_commit.as_str().to_string(),
        tree_oid,
        cargo_lock_sha256_hex,
        preparation_usage: usage,
    })
}

#[allow(dead_code)]
fn describe_run_primary(primary: &Result<RunscOutcome, RunFailure>) -> String {
    match primary {
        Ok(outcome) => format!(
            "exit={:?} timed_out={} stderr={:?}",
            outcome.exit,
            outcome.timed_out,
            String::from_utf8_lossy(&outcome.stderr)
        ),
        Err(failure) => failure.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "test-support")]
    use crate::gvisor::linux_capabilities::{ambient_capability_is_set, capability_is_inheritable};
    #[cfg(feature = "test-support")]
    use crate::user_namespace::UserNamespaceAllocator;
    #[cfg(feature = "test-support")]
    use std::os::unix::fs::MetadataExt;
    #[cfg(any(feature = "test-support", feature = "integration"))]
    use std::path::PathBuf;

    use crate::user_namespace::CheckoutSessionCleanup;

    use std::process::Command;

    use crate::gvisor::test_fixtures::*;

    use crate::workspace_intent::GitObjectFormat;
    use crate::ResourceLimits;
    use sha2::{Digest, Sha256};
    use std::ffi::CString;
    use std::io;
    use std::path::Path;

    use std::time::Duration;

    #[test]
    fn checkout_cleanup_plan_maps_every_disposition_to_its_one_safe_action() {
        assert_eq!(
            checkout_cleanup_plan(CheckoutSessionCleanup::NeverBound),
            CheckoutCleanupPlan::DeleteWorkspaceThenReleaseUnused,
            "a never-bound (Allocated) lease is released via release_unused, never release_prepared"
        );
        assert_eq!(
            checkout_cleanup_plan(CheckoutSessionCleanup::Prepared),
            CheckoutCleanupPlan::DeleteWorkspaceThenReleasePrepared,
            "a Prepared lease is released via release_prepared, never release_unused"
        );
        assert_eq!(
            checkout_cleanup_plan(CheckoutSessionCleanup::TeardownUnproven),
            CheckoutCleanupPlan::QuarantineBoth,
            "PreparationBound with unproven teardown is quarantined, never released"
        );
        assert_eq!(
            checkout_cleanup_plan(CheckoutSessionCleanup::Unreleasable),
            CheckoutCleanupPlan::QuarantineBoth,
            "an already-poisoned lease is quarantined, never released"
        );
        assert_eq!(
            checkout_cleanup_plan(CheckoutSessionCleanup::WorkloadBound),
            CheckoutCleanupPlan::AbandonBoth,
            "a bound workload's resources are owned by finalization - disposal abandons, never releases"
        );
    }

    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    enum RecordedCleanupOp {
        DeleteWorkspace,
        ReleaseUnused,
        ReleasePrepared,
        QuarantineWorkspace,
        QuarantineLease,
    }

    struct RecordingCleanupExecutor {
        ops: Vec<RecordedCleanupOp>,
        delete_proven: bool,
    }

    impl CheckoutCleanupExecutor for RecordingCleanupExecutor {
        fn delete_workspace(&mut self) -> (bool, Vec<String>) {
            self.ops.push(RecordedCleanupOp::DeleteWorkspace);
            (self.delete_proven, Vec::new())
        }
        fn release_unused(&mut self) -> Vec<String> {
            self.ops.push(RecordedCleanupOp::ReleaseUnused);
            Vec::new()
        }
        fn release_prepared(&mut self) -> Vec<String> {
            self.ops.push(RecordedCleanupOp::ReleasePrepared);
            Vec::new()
        }
        fn quarantine_workspace(&mut self) {
            self.ops.push(RecordedCleanupOp::QuarantineWorkspace);
        }
        fn quarantine_lease(&mut self) {
            self.ops.push(RecordedCleanupOp::QuarantineLease);
        }
    }

    fn trace(plan: CheckoutCleanupPlan, delete_proven: bool) -> Vec<RecordedCleanupOp> {
        let mut exec = RecordingCleanupExecutor {
            ops: Vec::new(),
            delete_proven,
        };
        execute_cleanup_plan(plan, &mut exec);
        exec.ops
    }

    #[test]
    fn each_cleanup_plan_executes_exactly_its_operation_sequence() {
        use CheckoutCleanupPlan::*;
        use RecordedCleanupOp::*;

        assert_eq!(
            trace(DeleteWorkspaceThenReleaseUnused, true),
            vec![DeleteWorkspace, ReleaseUnused],
            "NeverBound deletes the workspace then release_unused (never release_prepared)"
        );
        assert_eq!(
            trace(DeleteWorkspaceThenReleasePrepared, true),
            vec![DeleteWorkspace, ReleasePrepared],
            "Prepared deletes the workspace then release_prepared (never release_unused)"
        );
        assert_eq!(
            trace(DeleteWorkspaceThenReleaseUnused, false),
            vec![DeleteWorkspace, QuarantineLease],
            "an unproven delete must quarantine the lease, never release_unused it"
        );
        assert_eq!(
            trace(DeleteWorkspaceThenReleasePrepared, false),
            vec![DeleteWorkspace, QuarantineLease],
            "an unproven delete must quarantine the lease, never release_prepared it"
        );
        assert_eq!(
            trace(QuarantineBoth, true),
            vec![QuarantineWorkspace, QuarantineLease],
            "QuarantineBoth never deletes the workspace or releases the lease"
        );
        assert_eq!(
            trace(AbandonBoth, true),
            vec![QuarantineWorkspace, QuarantineLease],
            "AbandonBoth never deletes the workspace or releases the lease"
        );
    }

    mod checkout_preparation_5b2 {
        use super::*;
        #[cfg(feature = "integration")]
        use crate::gvisor::checkout_transport::fetch_checkout_pack;
        use crate::gvisor::checkout_transport_test_support::sha1_oid;
        #[cfg(feature = "integration")]
        use crate::{IdemToken, MeterTarget};

        #[test]
        fn expected_git_commit_id_accepts_a_valid_sha1_oid() {
            let oid = sha1_oid(0xab);
            let id = ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
            assert_eq!(id.as_str(), oid);
            assert_eq!(id.format(), GitObjectFormat::Sha1);
        }

        #[test]
        fn expected_git_commit_id_accepts_a_valid_sha256_oid() {
            let oid = "a".repeat(64);
            let id = ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha256).unwrap();
            assert_eq!(id.as_str(), oid);
        }

        #[test]
        fn expected_git_commit_id_refuses_the_wrong_width() {
            let err = ExpectedGitCommitId::new("a".repeat(64), GitObjectFormat::Sha1).unwrap_err();
            assert!(err.contains("40-character"));
        }

        #[test]
        fn expected_git_commit_id_refuses_non_hex() {
            let err = ExpectedGitCommitId::new("g".repeat(40), GitObjectFormat::Sha1).unwrap_err();
            assert!(err.contains("not lowercase hex"));
        }

        #[test]
        fn expected_git_commit_id_refuses_uppercase_hex() {
            let err = ExpectedGitCommitId::new("A".repeat(40), GitObjectFormat::Sha1).unwrap_err();
            assert!(err.contains("not lowercase hex"));
        }

        #[test]
        fn expected_git_commit_id_refuses_the_all_zero_null_id() {
            let err = ExpectedGitCommitId::new("0".repeat(40), GitObjectFormat::Sha1).unwrap_err();
            assert!(err.contains("all-zero null id"));
        }

        #[test]
        fn sha256_format_requests_the_object_format_capability() {
            assert_eq!(GitObjectFormat::Sha1.capability_token(), None);
            assert_eq!(
                GitObjectFormat::Sha256.capability_token(),
                Some("object-format=sha256")
            );
        }

        #[test]
        fn confirmation_line_parses_the_happy_path() {
            let commit = sha1_oid(0x78);
            let tree = sha1_oid(0x9a);
            let expected = ExpectedGitCommitId::new(commit.clone(), GitObjectFormat::Sha1).unwrap();
            let line = format!("{commit} {tree}\n");
            let got = parse_checkout_confirmation_line(line.as_bytes(), &expected).unwrap();
            assert_eq!(got, tree);
        }

        #[test]
        fn confirmation_line_refuses_a_mismatched_commit() {
            let commit = sha1_oid(0xbc);
            let other = sha1_oid(0xde);
            let tree = sha1_oid(0xf0);
            let expected = ExpectedGitCommitId::new(commit, GitObjectFormat::Sha1).unwrap();
            let line = format!("{other} {tree}\n");
            let err = parse_checkout_confirmation_line(line.as_bytes(), &expected).unwrap_err();
            assert!(err.contains("reports commit"));
        }

        #[test]
        fn confirmation_line_refuses_a_malformed_tree_oid() {
            let commit = sha1_oid(0x13);
            let expected = ExpectedGitCommitId::new(commit.clone(), GitObjectFormat::Sha1).unwrap();
            let line = format!("{commit} not-hex\n");
            let err = parse_checkout_confirmation_line(line.as_bytes(), &expected).unwrap_err();
            assert!(err.contains("not valid"));
        }

        #[test]
        fn confirmation_line_refuses_extra_fields() {
            let commit = sha1_oid(0x24);
            let tree = sha1_oid(0x35);
            let expected = ExpectedGitCommitId::new(commit.clone(), GitObjectFormat::Sha1).unwrap();
            let line = format!("{commit} {tree} extra\n");
            let err = parse_checkout_confirmation_line(line.as_bytes(), &expected).unwrap_err();
            assert!(err.contains("extra fields"));
        }

        #[test]
        fn confirmation_line_refuses_empty_output() {
            let commit = sha1_oid(0x46);
            let expected = ExpectedGitCommitId::new(commit, GitObjectFormat::Sha1).unwrap();
            let err = parse_checkout_confirmation_line(b"", &expected).unwrap_err();
            assert!(err.contains("missing the tree oid"));
        }

        fn valid_limits_for_tests() -> ResourceLimits {
            ResourceLimits {
                cpu_millis: 1000,
                mem_bytes: 256 << 20,
                disk_bytes: 1 << 30,
                tmpfs_bytes: 64 << 20,
                pids_max: 64,
                timeout_secs: 60,
            }
        }

        fn fake_pack_for_tests() -> PrefetchedCheckoutPack {
            PrefetchedCheckoutPack {
                file: tempfile_for_checkout_pack().unwrap().into_inner().unwrap(),
                shallow: false,
            }
        }

        #[test]
        fn checkout_preparation_spec_new_refuses_zero_pids_max() {
            let pack = fake_pack_for_tests();
            let expected = ExpectedGitCommitId::new(sha1_oid(0xc1), GitObjectFormat::Sha1).unwrap();
            let mut limits = valid_limits_for_tests();
            limits.pids_max = 0;
            let err = CheckoutPreparationSpec::new(expected, pack, limits).unwrap_err();
            assert!(err.contains("pids_max"));
        }

        #[test]
        fn checkout_preparation_spec_new_refuses_zero_timeout() {
            let pack = fake_pack_for_tests();
            let expected = ExpectedGitCommitId::new(sha1_oid(0xc2), GitObjectFormat::Sha1).unwrap();
            let mut limits = valid_limits_for_tests();
            limits.timeout_secs = 0;
            let err = CheckoutPreparationSpec::new(expected, pack, limits).unwrap_err();
            assert!(err.contains("timeout_secs"));
        }

        #[test]
        fn checkout_preparation_spec_new_accepts_valid_limits() {
            let pack = fake_pack_for_tests();
            let expected = ExpectedGitCommitId::new(sha1_oid(0xc3), GitObjectFormat::Sha1).unwrap();
            CheckoutPreparationSpec::new(expected, pack, valid_limits_for_tests())
                .expect("valid limits must be accepted");
        }

        fn drill_git_ok(args: &[&str], cwd: &Path) {
            let out = Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .expect("run host git");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        fn drill_git_rev_parse_head(cwd: &Path) -> String {
            let out = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(cwd)
                .output()
                .expect("git rev-parse HEAD");
            assert!(out.status.success());
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }

        fn run_gitlink_check(repo: &Path, oid: &str) -> std::process::Output {
            Command::new("sh")
                .arg("-c")
                .arg(GITLINK_CHECK_SNIPPET_FOR_TESTS)
                .arg("sh")
                .arg(oid)
                .current_dir(repo)
                .output()
                .expect("run sh -c <gitlink check snippet>")
        }

        #[test]
        #[cfg_attr(
            not(feature = "privileged-host-tests"),
            ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
        )]
        fn checkout_script_gitlink_check_passes_a_clean_commit() {
            let repo = temp_dir_for("gitlink-check-clean");
            drill_git_ok(&["init", "-q", "-b", "main"], &repo);
            drill_git_ok(&["config", "user.email", "t@t.t"], &repo);
            drill_git_ok(&["config", "user.name", "t"], &repo);
            std::fs::write(repo.join("f.txt"), b"hi\n").unwrap();
            drill_git_ok(&["add", "f.txt"], &repo);
            drill_git_ok(
                &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "clean"],
                &repo,
            );
            let oid = drill_git_rev_parse_head(&repo);
            let output = run_gitlink_check(&repo, &oid);
            assert!(
                output.status.success(),
                "a clean commit must pass: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ok");
            let _ = std::fs::remove_dir_all(&repo);
        }

        #[test]
        #[cfg_attr(
            not(feature = "privileged-host-tests"),
            ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
        )]
        fn checkout_script_gitlink_check_refuses_a_gitlink() {
            let repo = temp_dir_for("gitlink-check-refuses");
            drill_git_ok(&["init", "-q", "-b", "main"], &repo);
            drill_git_ok(&["config", "user.email", "t@t.t"], &repo);
            drill_git_ok(&["config", "user.name", "t"], &repo);
            std::fs::write(repo.join("f.txt"), b"hi\n").unwrap();
            drill_git_ok(&["add", "f.txt"], &repo);
            drill_git_ok(
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    "160000,1111111111111111111111111111111111111111,sub",
                ],
                &repo,
            );
            drill_git_ok(
                &[
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-q",
                    "-m",
                    "has a gitlink",
                ],
                &repo,
            );
            let oid = drill_git_rev_parse_head(&repo);
            let output = run_gitlink_check(&repo, &oid);
            assert!(
                !output.status.success(),
                "a commit with a gitlink must be refused"
            );
            assert!(String::from_utf8_lossy(&output.stderr).contains("gitlinks"));
            let _ = std::fs::remove_dir_all(&repo);
        }

        #[test]
        #[cfg_attr(
            not(feature = "privileged-host-tests"),
            ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
        )]
        fn checkout_script_gitlink_check_fails_closed_when_ls_tree_itself_fails() {
            let repo = temp_dir_for("gitlink-check-ls-tree-fails");
            drill_git_ok(&["init", "-q", "-b", "main"], &repo);
            let bogus_oid = "0".repeat(40);
            let output = run_gitlink_check(&repo, &bogus_oid);
            assert!(
                !output.status.success(),
                "an ls-tree failure must be a hard failure"
            );
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("git ls-tree failed"),
                "must fail on the ls-tree error, never silently pass as 'no gitlinks': stderr={}",
                String::from_utf8_lossy(&output.stderr)
            );
            let _ = std::fs::remove_dir_all(&repo);
        }

        #[test]
        fn verify_workspace_head_accepts_an_exact_match() {
            let ws = temp_dir_for("ok");
            let oid = sha1_oid(0x57);
            std::fs::create_dir_all(ws.join(".git")).unwrap();
            std::fs::write(ws.join(".git/HEAD"), format!("{oid}\n")).unwrap();
            let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
            verify_workspace_head_no_follow(&ws, &expected).unwrap();
            let _ = std::fs::remove_dir_all(&ws);
        }

        #[test]
        fn verify_workspace_head_refuses_a_mismatch() {
            let ws = temp_dir_for("mismatch");
            let oid = sha1_oid(0x68);
            let other = sha1_oid(0x79);
            std::fs::create_dir_all(ws.join(".git")).unwrap();
            std::fs::write(ws.join(".git/HEAD"), format!("{other}\n")).unwrap();
            let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
            let err = verify_workspace_head_no_follow(&ws, &expected).unwrap_err();
            assert!(err.contains("does not exactly match"));
            let _ = std::fs::remove_dir_all(&ws);
        }

        #[test]
        #[cfg_attr(
            not(feature = "privileged-host-tests"),
            ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
        )]
        fn verify_workspace_head_refuses_a_symlinked_git_directory() {
            let ws = temp_dir_for("symlink-git");
            let real = temp_dir_for("symlink-git-target");
            std::fs::write(real.join("HEAD"), "irrelevant\n").unwrap();
            std::os::unix::fs::symlink(&real, ws.join(".git")).unwrap();
            let oid = sha1_oid(0x8a);
            let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
            let err = verify_workspace_head_no_follow(&ws, &expected).unwrap_err();
            assert!(err.contains(".git is not a real directory"));
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&real);
        }

        #[test]
        fn verify_workspace_head_refuses_a_symlinked_head_file() {
            let ws = temp_dir_for("symlink-head");
            std::fs::create_dir_all(ws.join(".git")).unwrap();
            let oid = sha1_oid(0x9b);
            let real_head = ws.join(".git/REAL_HEAD");
            std::fs::write(&real_head, format!("{oid}\n")).unwrap();
            std::os::unix::fs::symlink(&real_head, ws.join(".git/HEAD")).unwrap();
            let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
            let err = verify_workspace_head_no_follow(&ws, &expected).unwrap_err();
            assert!(err.contains(".git/HEAD is not a real regular file"));
            let _ = std::fs::remove_dir_all(&ws);
        }

        #[test]
        fn verify_workspace_head_refuses_a_fifo_without_blocking() {
            let ws = temp_dir_for("fifo-head");
            std::fs::create_dir_all(ws.join(".git")).unwrap();
            let head_path = ws.join(".git/HEAD");
            let head_c = CString::new(head_path.as_os_str().as_encoded_bytes()).unwrap();
            let rc = unsafe { libc::mkfifo(head_c.as_ptr(), 0o600) };
            assert_eq!(rc, 0, "mkfifo must succeed: {}", io::Error::last_os_error());
            let oid = sha1_oid(0x9c);
            let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
            let ws_for_thread = ws.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let result = verify_workspace_head_no_follow(&ws_for_thread, &expected);
                let _ = tx.send(result);
            });
            let result = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("verify_workspace_head_no_follow must not block on a guest-planted FIFO");
            let err = result.unwrap_err();
            assert!(err.contains(".git/HEAD is not a real regular file"));
            let _ = std::fs::remove_dir_all(&ws);
        }

        #[test]
        fn verify_workspace_head_refuses_an_implausibly_large_head_file() {
            let ws = temp_dir_for("oversized-head");
            std::fs::create_dir_all(ws.join(".git")).unwrap();
            std::fs::write(ws.join(".git/HEAD"), "a".repeat(9000)).unwrap();
            let oid = sha1_oid(0xac);
            let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
            let err = verify_workspace_head_no_follow(&ws, &expected).unwrap_err();
            assert!(err.contains("implausibly large"));
            let _ = std::fs::remove_dir_all(&ws);
        }

        #[test]
        fn verify_workspace_head_refuses_a_missing_git_directory() {
            let ws = temp_dir_for("no-git");
            let oid = sha1_oid(0xbd);
            let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
            let err = verify_workspace_head_no_follow(&ws, &expected).unwrap_err();
            assert!(err.contains(".git is not a real directory"));
            let _ = std::fs::remove_dir_all(&ws);
        }

        #[test]
        fn hash_workspace_cargo_lock_computes_a_real_sha256() {
            let ws = temp_dir_for("cargo-lock-ok");
            std::fs::write(ws.join("Cargo.lock"), b"lockfile bytes").unwrap();
            let hex = hash_workspace_cargo_lock_no_follow(&ws).unwrap();
            let mut hasher = Sha256::new();
            hasher.update(b"lockfile bytes");
            let expected_hex = hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            assert_eq!(hex, expected_hex);
            let _ = std::fs::remove_dir_all(&ws);
        }

        #[test]
        fn hash_workspace_cargo_lock_refuses_absence() {
            let ws = temp_dir_for("cargo-lock-absent");
            let err = hash_workspace_cargo_lock_no_follow(&ws).unwrap_err();
            assert!(err.contains("Cargo.lock is not present"));
            let _ = std::fs::remove_dir_all(&ws);
        }

        #[test]
        fn hash_workspace_cargo_lock_refuses_a_symlink() {
            let ws = temp_dir_for("cargo-lock-symlink");
            let real = temp_dir_for("cargo-lock-symlink-target");
            std::fs::write(real.join("real-lock"), b"lockfile bytes").unwrap();
            std::os::unix::fs::symlink(real.join("real-lock"), ws.join("Cargo.lock")).unwrap();
            let err = hash_workspace_cargo_lock_no_follow(&ws).unwrap_err();
            assert!(err.contains("Cargo.lock is not present"));
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&real);
        }

        #[cfg(feature = "test-support")]
        #[test]
        fn host_verifier_reads_subuid_owned_umask_077_checkout_without_normalizing_ownership() {
            let initial = current_thread_capabilities().unwrap();
            if !capability_is_permitted(&initial, CAP_DAC_READ_SEARCH_NUMBER) {
                if std::env::var("MYELIN_REQUIRE_DAC_READ_SEARCH_TEST").as_deref() == Ok("1") {
                    panic!(
                        "MYELIN_REQUIRE_DAC_READ_SEARCH_TEST=1 but CAP_DAC_READ_SEARCH is absent \
                         from the test process's permitted set"
                    );
                }
                eprintln!(
                    "host_verifier_reads_subuid_owned_umask_077_checkout_without_normalizing_ownership: \
                     SKIPPED - rerun under the production-shaped ambient capability grant with \
                     MYELIN_REQUIRE_DAC_READ_SEARCH_TEST=1 to hard-require this privileged drill"
                );
                return;
            }
            prepare_checkout_host_verification_capability(true)
                .expect("the privileged regression unit must supply CAP_DAC_READ_SEARCH");
            let Some((allocator, leases_dir)) =
                real_userns_allocator_for_tests("host-verifier-subuid")
            else {
                panic!("the privileged regression unit requires a usable subordinate uid/gid");
            };
            let lease = allocator.lease().expect("lease a real subordinate uid/gid");
            let subuid = lease.host_uid();
            let subgid = lease.host_gid();
            assert_ne!(subuid, unsafe { libc::geteuid() });

            let ws = temp_dir_for("subuid-0700");
            std::fs::create_dir(ws.join(".git")).unwrap();
            let oid = sha1_oid(0xce);
            std::fs::write(ws.join(".git/HEAD"), format!("{oid}\n")).unwrap();
            let lock_bytes = b"# subuid regression lockfile\n";
            std::fs::write(ws.join("Cargo.lock"), lock_bytes).unwrap();

            let ws_fd = std::fs::File::open(&ws).unwrap();
            let git_fd = std::fs::File::open(ws.join(".git")).unwrap();
            let head_fd = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(ws.join(".git/HEAD"))
                .unwrap();
            let lock_fd = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(ws.join("Cargo.lock"))
                .unwrap();
            let transfer_to_subuid = |file: &std::fs::File, mode: u32| {
                assert_eq!(unsafe { libc::fchmod(file.as_raw_fd(), mode) }, 0);
                assert_eq!(unsafe { libc::fchown(file.as_raw_fd(), subuid, subgid) }, 0);
            };
            let restore_to_runner = |file: &std::fs::File, mode: u32| {
                assert_eq!(
                    unsafe { libc::fchown(file.as_raw_fd(), libc::geteuid(), libc::getegid()) },
                    0
                );
                assert_eq!(unsafe { libc::fchmod(file.as_raw_fd(), mode) }, 0);
            };
            transfer_to_subuid(&head_fd, 0o600);
            transfer_to_subuid(&lock_fd, 0o600);
            transfer_to_subuid(&git_fd, 0o700);
            transfer_to_subuid(&ws_fd, 0o755);

            let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
            let ordinary_dac_error = verify_workspace_head_no_follow(&ws, &expected).unwrap_err();
            let verified_digest = verify_materialized_checkout_no_follow(&ws, &expected);
            let ws_meta = ws_fd.metadata().unwrap();
            let git_meta = git_fd.metadata().unwrap();
            let head_meta = head_fd.metadata().unwrap();
            let lock_meta = lock_fd.metadata().unwrap();
            let post_scope_caps = current_thread_capabilities().unwrap();
            let post_scope_ambient = ambient_capability_is_set(CAP_DAC_READ_SEARCH_NUMBER).unwrap();

            restore_to_runner(&head_fd, 0o600);
            restore_to_runner(&lock_fd, 0o600);
            restore_to_runner(&git_fd, 0o700);
            restore_to_runner(&ws_fd, 0o755);
            drop((head_fd, lock_fd, git_fd, ws_fd));
            lease.release_unused().expect("release unused subuid lease");
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&leases_dir);

            assert!(
                ordinary_dac_error.contains("Permission denied"),
                "ordinary host DAC must reproduce the exact .git traversal failure: \
                 {ordinary_dac_error}"
            );
            let mut hasher = Sha256::new();
            hasher.update(lock_bytes);
            let expected_digest = hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(verified_digest.unwrap(), expected_digest);
            for (label, metadata, mode) in [
                ("workspace", ws_meta, 0o755),
                (".git", git_meta, 0o700),
                (".git/HEAD", head_meta, 0o600),
                ("Cargo.lock", lock_meta, 0o600),
            ] {
                assert_eq!(
                    metadata.uid(),
                    subuid,
                    "{label} owner must remain the subuid"
                );
                assert_eq!(
                    metadata.gid(),
                    subgid,
                    "{label} group must remain the subgid"
                );
                assert_eq!(
                    metadata.mode() & 0o777,
                    mode,
                    "{label} mode must not be widened"
                );
            }
            assert!(
                !capability_is_effective(&post_scope_caps, CAP_DAC_READ_SEARCH_NUMBER),
                "CAP_DAC_READ_SEARCH must be withdrawn after verification"
            );
            assert!(
                !capability_is_inheritable(&post_scope_caps, CAP_DAC_READ_SEARCH_NUMBER),
                "CAP_DAC_READ_SEARCH must never remain inheritable"
            );
            assert!(
                !post_scope_ambient,
                "runsc/child execs must never inherit the cap"
            );
        }

        #[cfg(feature = "integration")]
        fn drill_runsc_bin() -> Option<String> {
            let bin = std::env::var("MYELIN_RUNSC_BIN").unwrap_or_else(|_| "runsc".to_string());
            if bin.contains('/') {
                return Path::new(&bin).exists().then_some(bin);
            }
            let path = std::env::var("PATH").ok()?;
            for dir in path.split(':') {
                if Path::new(dir).join(&bin).exists() {
                    return Some(bin);
                }
            }
            None
        }

        #[cfg(feature = "integration")]
        fn drill_copy_file(src: &Path, dst: &Path) {
            if let Some(p) = dst.parent() {
                std::fs::create_dir_all(p).expect("mkdir -p");
            }
            std::fs::copy(src, dst).unwrap_or_else(|e| panic!("copy {src:?} -> {dst:?}: {e}"));
        }

        #[cfg(feature = "integration")]
        fn drill_stage_lib(rootfs: &Path, soname: &str, host_path: &str) {
            let real =
                std::fs::canonicalize(host_path).unwrap_or_else(|_| PathBuf::from(host_path));
            let real_name = real.file_name().unwrap().to_string_lossy().to_string();
            for libdir in ["usr/lib", "lib"] {
                let dst_real = rootfs.join(libdir).join(&real_name);
                drill_copy_file(&real, &dst_real);
                let link = rootfs.join(libdir).join(soname);
                let _ = std::fs::remove_file(&link);
                std::os::unix::fs::symlink(&real_name, &link).expect("soname symlink");
            }
        }

        #[cfg(feature = "integration")]
        fn drill_stage_git_rootfs(base: &Path) -> PathBuf {
            let staged = std::env::temp_dir().join(format!(
                "myelin-checkout-drill-git-rootfs-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&staged);
            let st = Command::new("cp")
                .arg("-a")
                .arg(format!("{}/.", base.display()))
                .arg(&staged)
                .status()
                .expect("cp -a base rootfs");
            assert!(st.success(), "cp -a base rootfs failed");
            drill_copy_file(Path::new("/usr/bin/git"), &staged.join("usr/bin/git"));
            drill_stage_lib(&staged, "libpcre2-8.so.0", "/usr/lib/libpcre2-8.so.0");
            drill_stage_lib(&staged, "libz-ng.so.2", "/usr/lib/libz-ng.so.2");
            let core = staged.join("usr/lib/git-core");
            std::fs::create_dir_all(&core).expect("mkdir git-core");
            for helper in ["git-upload-pack", "git-receive-pack"] {
                let link = core.join(helper);
                let _ = std::fs::remove_file(&link);
                std::os::unix::fs::symlink("../../bin/git", &link)
                    .expect("git-core helper symlink");
            }
            for destination in ["tmp", "workspace", "repo", "quarantine"] {
                std::fs::create_dir_all(staged.join(destination))
                    .unwrap_or_else(|error| panic!("mkdir /{destination} mount point: {error}"));
            }
            staged
        }

        #[cfg(feature = "integration")]
        fn drill_git_rootfs() -> Option<PathBuf> {
            static STAGED: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
            STAGED
                .get_or_init(|| {
                    let base = resolved_gvisor_rootfs();
                    if !base.exists() {
                        return None;
                    }
                    let staged = drill_stage_git_rootfs(&base);
                    std::env::set_var(ENV_GVISOR_GIT_ROOTFS, &staged);
                    Some(staged)
                })
                .clone()
        }

        #[cfg(feature = "integration")]
        fn drill_run_git(args: &[&str], cwd: Option<&Path>) {
            let mut c = Command::new("git");
            c.args(args);
            if let Some(d) = cwd {
                c.current_dir(d);
            }
            let out = c.output().expect("run host git");
            assert!(
                out.status.success(),
                "host git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        #[cfg(feature = "integration")]
        fn drill_rev_parse(cwd: &Path, rev: &str) -> String {
            let out = Command::new("git")
                .args(["rev-parse", rev])
                .current_dir(cwd)
                .output()
                .expect("git rev-parse");
            assert!(out.status.success());
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }

        #[cfg(feature = "integration")]
        fn drill_make_repo_with_two_commits(
            root: &Path,
            tenant: &str,
            region: &str,
            repo: &str,
        ) -> (String, String) {
            let bare =
                resolve_bare_repo_path(root, tenant, region, repo).expect("resolve bare path");
            std::fs::create_dir_all(bare.parent().unwrap()).expect("mkdir repo parent");
            drill_run_git(&["init", "-q", "--bare", &bare.to_string_lossy()], None);
            let work = root.join("work");
            std::fs::create_dir_all(&work).expect("mkdir work");
            drill_run_git(&["init", "-q", "-b", "main"], Some(&work));
            drill_run_git(&["config", "user.email", "t@t.t"], Some(&work));
            drill_run_git(&["config", "user.name", "t"], Some(&work));
            std::fs::write(work.join("Cargo.lock"), b"# drill fixture lockfile\n").unwrap();
            std::fs::write(work.join("f.txt"), b"first\n").unwrap();
            drill_run_git(&["add", "Cargo.lock", "f.txt"], Some(&work));
            drill_run_git(
                &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "first"],
                Some(&work),
            );
            let older = drill_rev_parse(&work, "HEAD");
            std::fs::write(work.join("f.txt"), b"second\n").unwrap();
            drill_run_git(&["add", "f.txt"], Some(&work));
            drill_run_git(
                &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "second"],
                Some(&work),
            );
            let newer = drill_rev_parse(&work, "HEAD");
            drill_run_git(
                &["push", "-q", &bare.to_string_lossy(), "main"],
                Some(&work),
            );
            (older, newer)
        }

        #[test]
        #[cfg(feature = "integration")]
        fn checkout_preparation_runs_end_to_end_through_real_git_wire_and_runsc() {
            let _leases_dir_guard = USERNS_DRILL_LEASES_DIR_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(_runsc) = drill_runsc_bin() else {
                eprintln!("[checkout live drill] SKIP: `runsc` is not on PATH");
                return;
            };
            if let Err(e) = preflight_explicit_userns_policy(
                resolved_explicit_userns_helper_dir(),
                resolved_explicit_userns_runsc_root(),
            ) {
                eprintln!(
                    "[checkout live drill] SKIP: preflight_explicit_userns_policy failed: {e}"
                );
                return;
            }
            let Some(git_rootfs) = drill_git_rootfs() else {
                eprintln!(
                    "[checkout live drill] SKIP: base rootfs absent -- cannot stage a git-bearing \
                     rootfs"
                );
                return;
            };
            let leases_dir = match std::env::var(USERNS_DRILL_LEASES_DIR_ENV) {
                Ok(value) if !value.is_empty() => PathBuf::from(value),
                _ => {
                    eprintln!(
                        "[checkout live drill] SKIP: {USERNS_DRILL_LEASES_DIR_ENV} is not set -- \
                         needs an operator-provisioned leases directory, same STRICT contract as \
                         the Enabled activation drill (pre-existing, euid-owned, mode 0700 or \
                         stricter, non-writable-by-us ancestor chain)"
                    );
                    return;
                }
            };

            let tag = format!("{}-{}", std::process::id(), unique_suffix());
            let root = std::env::temp_dir().join(format!("myelin-checkout-drill-repo-{tag}"));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("mkdir drill repo root");
            let (older_oid, _newer_oid) =
                drill_make_repo_with_two_commits(&root, "acme", "fr-par", "widgets");

            let git_backend = GvisorBackend::new(test_registry());
            let hooks = ok_hooks();
            let expected = ExpectedGitCommitId::new(older_oid.clone(), GitObjectFormat::Sha1)
                .expect("older_oid is valid 40-hex");
            let checkout_limits = ResourceLimits {
                cpu_millis: 1000,
                mem_bytes: 256 << 20,
                disk_bytes: 256 << 20,
                tmpfs_bytes: 64 << 20,
                pids_max: 128,
                timeout_secs: 60,
            };
            let run_token = RunTokenCredential::new("drill-bearer", "drill-jti", 3600)
                .expect("a non-empty bearer/jti/positive ttl must construct");
            let meter_to = MeterTarget {
                reserve_id: "checkout-drill".to_string(),
            };
            let pack = fetch_checkout_pack(
                &git_backend,
                &hooks,
                &root,
                "acme",
                "fr-par",
                "widgets",
                &expected,
                checkout_limits,
                run_token,
                meter_to,
                IdemToken(format!("checkout-drill-{tag}")),
            )
            .expect(
                "fetch_checkout_pack must succeed once runsc/rootfs/repo prerequisites are \
                 configured -- reaching this point asserts the host IS correctly provisioned, so \
                 a failure here is a genuine regression",
            );

            let mut workspace_base_dir =
                std::env::home_dir().expect("HOME must be set for this test");
            workspace_base_dir.push(format!(
                ".local/state/myelin-checkout-drill-workspace-{tag}"
            ));
            let incident_sink: crate::workspace_manager::IncidentSink =
                Arc::new(|msg: &str| eprintln!("[checkout live drill incident] {msg}"));
            let enabled_backend = GvisorBackend::try_new(
                real_userns_drill_registry(&git_rootfs),
                GvisorWorkspaceConfig::Enabled {
                    base_dir: workspace_base_dir.clone(),
                    host_capacity_bytes: 1 << 30,
                    leases_dir,
                    min_pool_size: 1,
                },
                incident_sink,
            )
            .expect(
                "GvisorBackend::try_new(Enabled) must succeed once an operator-provisioned \
                 leases directory is configured",
            );
            let job_spec = spec(vec![]);
            let profile = HardeningProfile::derive(&job_spec);
            let container_id = format!("myelin-checkout-drill-{tag}");
            let (workspace_manager, userns_allocator) = match &enabled_backend.workspace_integration
            {
                WorkspaceIntegration::Enabled {
                    workspace_manager,
                    userns_allocator,
                } => (workspace_manager, userns_allocator),
                WorkspaceIntegration::Disabled => {
                    panic!("try_new(Enabled) must produce an Enabled workspace_integration")
                }
            };
            let (_discarded_cfg, mut context) = acquire_enabled_workspace(
                &job_spec,
                &profile,
                &container_id,
                git_rootfs.clone(),
                workspace_manager,
                userns_allocator,
                None,
            )
            .expect("acquiring a real workspace + userns lease must succeed on a healthy host");

            let checkout_spec = CheckoutPreparationSpec::new(expected, pack, checkout_limits)
                .expect("valid limits must construct a CheckoutPreparationSpec");
            let mut session = CheckoutPreparationSession::new();
            let evidence = run_checkout_preparation(
                &mut context.lease,
                &mut session,
                &context.workspace,
                checkout_spec,
            )
            .expect(
                "run_checkout_preparation must succeed once runsc/rootfs/workspace/lease \
                 prerequisites are configured -- reaching this point asserts the host IS \
                 correctly provisioned, so any failure here is a genuine regression",
            );

            assert_eq!(evidence.commit_hex(), older_oid);
            assert!(
                !evidence.cargo_lock_sha256_hex().is_empty(),
                "the drill repo's Cargo.lock must have been hashed"
            );

            let EnabledLaunchContext {
                workspace, lease, ..
            } = context;
            workspace_manager
                .delete_workspace(workspace)
                .expect("delete_workspace must succeed after a real, proven-clean checkout run");
            session
                .release_prepared(lease)
                .expect("release_prepared must succeed after a real, proven-clean checkout run");
            let _ = std::fs::remove_dir_all(&root);
            let _ = std::fs::remove_dir_all(&workspace_base_dir);
        }

        #[test]
        fn usage_from_runsc_outcome_ceils_wall_clock_and_floors_cpu_from_proc() {
            let outcome = RunscOutcome {
                exit: Some(0),
                timed_out: false,
                stdout: Vec::new(),
                stdout_truncated: false,
                stderr: Vec::new(),
                wall: Duration::from_millis(1500),
                cpu_seconds: None,
                stream_error: None,
            };
            let usage = usage_from_runsc_outcome(1 << 20, &outcome);
            assert_eq!(usage.cpu_seconds, 2);
            assert_eq!(usage.mem_byte_seconds, (1 << 20) * 2);
        }

        #[cfg(feature = "test-support")]
        fn real_lease_for_eval_test(tag: &str) -> Option<(UserNamespaceAllocator, PathBuf)> {
            real_userns_allocator_for_tests(tag)
        }

        #[cfg(feature = "test-support")]
        #[test]
        fn evaluate_checkout_finalization_never_confirms_when_teardown_is_unproven() {
            let Some((allocator, leases_dir)) = real_lease_for_eval_test("eval-teardown-unproven")
            else {
                eprintln!("[checkout eval test] SKIP: no usable /etc/subuid|subgid range");
                return;
            };
            let mut lease = allocator.lease().unwrap();
            let mut session = CheckoutPreparationSession::new();
            session
                .bind_preparation(&mut lease, "c1".to_string(), (11, 11), (22, 22))
                .expect("bind_preparation must succeed");
            let finalization: RuntimeFinalization<Result<RunscOutcome, RunFailure>> =
                RuntimeFinalization::Failed {
                    primary: Ok(outcome(b"whatever the guest printed", b"")),
                    teardown: RuntimeTeardownError {
                        issues: vec![RuntimeTeardownIssue::Cgroup(
                            CgroupQuiescenceError::StillPopulated {
                                waited: Duration::from_secs(1),
                            },
                        )],
                    },
                };
            let ws = temp_dir_for("teardown-unproven-ws");
            let expected = ExpectedGitCommitId::new(sha1_oid(0xa1), GitObjectFormat::Sha1).unwrap();
            let result = evaluate_checkout_finalization(
                finalization,
                &mut lease,
                &mut session,
                1 << 20,
                &expected,
                &ws,
            );
            match result {
                Err(CheckoutPreparationError::TeardownUnproven { .. }) => {}
                other => panic!("expected TeardownUnproven, got {other:?}"),
            }
            let nonce = lease.nonce_for_tests();
            session
                .confirm_prepared(
                    &mut lease,
                    PreparationQuiescenceProof::assert_for_tests(
                        nonce,
                        "c1".to_string(),
                        (11, 11),
                        (22, 22),
                    ),
                )
                .expect(
                    "session must still be PreparationBound -- confirm_prepared was never \
                     attempted on a teardown-unproven finalization",
                );
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&leases_dir);
        }

        #[cfg(feature = "test-support")]
        #[test]
        fn evaluate_checkout_finalization_confirms_before_checking_exit_status() {
            let Some((allocator, leases_dir)) = real_lease_for_eval_test("eval-bad-exit") else {
                eprintln!("[checkout eval test] SKIP: no usable /etc/subuid|subgid range");
                return;
            };
            let mut lease = allocator.lease().unwrap();
            let mut session = CheckoutPreparationSession::new();
            session
                .bind_preparation(&mut lease, "c2".to_string(), (33, 33), (44, 44))
                .expect("bind_preparation must succeed");
            let mut bad_outcome = outcome(b"", b"checkout script failed");
            bad_outcome.exit = Some(1);
            let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
                "c2".to_string(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: (33, 33),
                },
                CgroupQuiescenceEvidence::assert_for_tests((44, 44)),
            );
            let finalization: RuntimeFinalization<Result<RunscOutcome, RunFailure>> =
                RuntimeFinalization::Finalized(FinalizedRun {
                    primary: Ok(bad_outcome),
                    evidence,
                });
            let ws = temp_dir_for("bad-exit-ws");
            let expected = ExpectedGitCommitId::new(sha1_oid(0xa2), GitObjectFormat::Sha1).unwrap();
            let result = evaluate_checkout_finalization(
                finalization,
                &mut lease,
                &mut session,
                1 << 20,
                &expected,
                &ws,
            );
            match result {
                Err(CheckoutPreparationError::RejectedAfterQuiescence { .. }) => {}
                other => panic!("expected RejectedAfterQuiescence, got {other:?}"),
            }
            session.release_prepared(lease).expect(
                "session must have reached Prepared -- confirm_prepared must run before the \
                 exit-status check, regardless of its outcome",
            );
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&leases_dir);
        }

        #[cfg(feature = "test-support")]
        #[test]
        fn evaluate_checkout_finalization_rejects_a_truncated_confirmation_line() {
            let Some((allocator, leases_dir)) = real_lease_for_eval_test("eval-stdout-truncated")
            else {
                eprintln!("[checkout eval test] SKIP: no usable /etc/subuid|subgid range");
                return;
            };
            let mut lease = allocator.lease().unwrap();
            let mut session = CheckoutPreparationSession::new();
            session
                .bind_preparation(&mut lease, "c2t".to_string(), (34, 34), (45, 45))
                .expect("bind_preparation must succeed");
            let mut truncated_outcome = outcome(b"partial-line-that-got-cut", b"");
            truncated_outcome.exit = Some(0);
            truncated_outcome.stdout_truncated = true;
            let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
                "c2t".to_string(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: (34, 34),
                },
                CgroupQuiescenceEvidence::assert_for_tests((45, 45)),
            );
            let finalization: RuntimeFinalization<Result<RunscOutcome, RunFailure>> =
                RuntimeFinalization::Finalized(FinalizedRun {
                    primary: Ok(truncated_outcome),
                    evidence,
                });
            let ws = temp_dir_for("stdout-truncated-ws");
            let expected = ExpectedGitCommitId::new(sha1_oid(0xb1), GitObjectFormat::Sha1).unwrap();
            let result = evaluate_checkout_finalization(
                finalization,
                &mut lease,
                &mut session,
                1 << 20,
                &expected,
                &ws,
            );
            match result {
                Err(CheckoutPreparationError::RejectedAfterQuiescence { message, .. }) => {
                    assert!(message.contains("truncated"));
                }
                other => panic!("expected RejectedAfterQuiescence, got {other:?}"),
            }
            session.release_prepared(lease).expect(
                "session must have reached Prepared despite the truncated confirmation output",
            );
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&leases_dir);
        }

        #[cfg(feature = "test-support")]
        #[test]
        fn evaluate_checkout_finalization_rejects_a_stream_error() {
            let Some((allocator, leases_dir)) = real_lease_for_eval_test("eval-stream-error")
            else {
                eprintln!("[checkout eval test] SKIP: no usable /etc/subuid|subgid range");
                return;
            };
            let mut lease = allocator.lease().unwrap();
            let mut session = CheckoutPreparationSession::new();
            session
                .bind_preparation(&mut lease, "c2s".to_string(), (35, 35), (46, 46))
                .expect("bind_preparation must succeed");
            let mut stream_error_outcome = outcome(b"whatever was captured before the error", b"");
            stream_error_outcome.exit = Some(0);
            stream_error_outcome.stream_error = Some("durable log sink write failed".to_string());
            let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
                "c2s".to_string(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: (35, 35),
                },
                CgroupQuiescenceEvidence::assert_for_tests((46, 46)),
            );
            let finalization: RuntimeFinalization<Result<RunscOutcome, RunFailure>> =
                RuntimeFinalization::Finalized(FinalizedRun {
                    primary: Ok(stream_error_outcome),
                    evidence,
                });
            let ws = temp_dir_for("stream-error-ws");
            let expected = ExpectedGitCommitId::new(sha1_oid(0xb2), GitObjectFormat::Sha1).unwrap();
            let result = evaluate_checkout_finalization(
                finalization,
                &mut lease,
                &mut session,
                1 << 20,
                &expected,
                &ws,
            );
            match result {
                Err(CheckoutPreparationError::RejectedAfterQuiescence { message, .. }) => {
                    assert!(message.contains("durable log sink write failed"));
                }
                other => panic!("expected RejectedAfterQuiescence, got {other:?}"),
            }
            session
                .release_prepared(lease)
                .expect("session must have reached Prepared despite the stream error");
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&leases_dir);
        }

        #[cfg(feature = "test-support")]
        #[test]
        fn evaluate_checkout_finalization_confirms_before_checking_the_confirmation_line() {
            let Some((allocator, leases_dir)) = real_lease_for_eval_test("eval-bad-confirm-line")
            else {
                eprintln!("[checkout eval test] SKIP: no usable /etc/subuid|subgid range");
                return;
            };
            let mut lease = allocator.lease().unwrap();
            let mut session = CheckoutPreparationSession::new();
            session
                .bind_preparation(&mut lease, "c3".to_string(), (55, 55), (66, 66))
                .expect("bind_preparation must succeed");
            let mut ok_exit_bad_output = outcome(b"not the expected confirmation line\n", b"");
            ok_exit_bad_output.exit = Some(0);
            let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
                "c3".to_string(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: (55, 55),
                },
                CgroupQuiescenceEvidence::assert_for_tests((66, 66)),
            );
            let finalization: RuntimeFinalization<Result<RunscOutcome, RunFailure>> =
                RuntimeFinalization::Finalized(FinalizedRun {
                    primary: Ok(ok_exit_bad_output),
                    evidence,
                });
            let ws = temp_dir_for("bad-confirm-line-ws");
            let expected = ExpectedGitCommitId::new(sha1_oid(0xa3), GitObjectFormat::Sha1).unwrap();
            let result = evaluate_checkout_finalization(
                finalization,
                &mut lease,
                &mut session,
                1 << 20,
                &expected,
                &ws,
            );
            match result {
                Err(CheckoutPreparationError::RejectedAfterQuiescence { .. }) => {}
                other => panic!("expected RejectedAfterQuiescence, got {other:?}"),
            }
            session
                .release_prepared(lease)
                .expect("session must have reached Prepared despite the bad confirmation line");
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&leases_dir);
        }

        #[cfg(feature = "test-support")]
        #[test]
        fn evaluate_checkout_finalization_confirms_before_checking_the_host_head_reread() {
            let Some((allocator, leases_dir)) = real_lease_for_eval_test("eval-bad-host-head")
            else {
                eprintln!("[checkout eval test] SKIP: no usable /etc/subuid|subgid range");
                return;
            };
            let mut lease = allocator.lease().unwrap();
            let mut session = CheckoutPreparationSession::new();
            session
                .bind_preparation(&mut lease, "c4".to_string(), (77, 77), (88, 88))
                .expect("bind_preparation must succeed");
            let expected = ExpectedGitCommitId::new(sha1_oid(0xa4), GitObjectFormat::Sha1).unwrap();
            let tree = sha1_oid(0xa5);
            let mut good_exit_good_line =
                outcome(format!("{} {tree}\n", expected.as_str()).as_bytes(), b"");
            good_exit_good_line.exit = Some(0);
            let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
                "c4".to_string(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: (77, 77),
                },
                CgroupQuiescenceEvidence::assert_for_tests((88, 88)),
            );
            let finalization: RuntimeFinalization<Result<RunscOutcome, RunFailure>> =
                RuntimeFinalization::Finalized(FinalizedRun {
                    primary: Ok(good_exit_good_line),
                    evidence,
                });
            let ws = temp_dir_for("bad-host-head-ws");
            std::fs::create_dir_all(ws.join(".git")).unwrap();
            std::fs::write(ws.join(".git/HEAD"), format!("{}\n", sha1_oid(0xa6))).unwrap();
            let result = evaluate_checkout_finalization(
                finalization,
                &mut lease,
                &mut session,
                1 << 20,
                &expected,
                &ws,
            );
            match result {
                Err(CheckoutPreparationError::RejectedAfterQuiescence { message, .. }) => {
                    assert!(message.contains("host-side HEAD re-verification disagreed"));
                }
                other => panic!("expected RejectedAfterQuiescence, got {other:?}"),
            }
            session
                .release_prepared(lease)
                .expect("session must have reached Prepared despite the host HEAD disagreement");
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&leases_dir);
        }

        #[cfg(feature = "test-support")]
        #[test]
        fn evaluate_checkout_finalization_confirms_before_checking_for_cargo_lock() {
            let Some((allocator, leases_dir)) = real_lease_for_eval_test("eval-missing-cargo-lock")
            else {
                eprintln!("[checkout eval test] SKIP: no usable /etc/subuid|subgid range");
                return;
            };
            let mut lease = allocator.lease().unwrap();
            let mut session = CheckoutPreparationSession::new();
            session
                .bind_preparation(&mut lease, "c4b".to_string(), (78, 78), (89, 89))
                .expect("bind_preparation must succeed");
            let expected = ExpectedGitCommitId::new(sha1_oid(0xb3), GitObjectFormat::Sha1).unwrap();
            let tree = sha1_oid(0xb4);
            let mut good_exit_good_line =
                outcome(format!("{} {tree}\n", expected.as_str()).as_bytes(), b"");
            good_exit_good_line.exit = Some(0);
            let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
                "c4b".to_string(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: (78, 78),
                },
                CgroupQuiescenceEvidence::assert_for_tests((89, 89)),
            );
            let finalization: RuntimeFinalization<Result<RunscOutcome, RunFailure>> =
                RuntimeFinalization::Finalized(FinalizedRun {
                    primary: Ok(good_exit_good_line),
                    evidence,
                });
            let ws = temp_dir_for("missing-cargo-lock-ws");
            std::fs::create_dir_all(ws.join(".git")).unwrap();
            std::fs::write(ws.join(".git/HEAD"), format!("{}\n", expected.as_str())).unwrap();
            let result = evaluate_checkout_finalization(
                finalization,
                &mut lease,
                &mut session,
                1 << 20,
                &expected,
                &ws,
            );
            match result {
                Err(CheckoutPreparationError::RejectedAfterQuiescence { message, .. }) => {
                    assert!(message.contains("could not hash the materialized Cargo.lock"));
                }
                other => panic!("expected RejectedAfterQuiescence, got {other:?}"),
            }
            session
                .release_prepared(lease)
                .expect("session must have reached Prepared despite the missing Cargo.lock");
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&leases_dir);
        }

        #[cfg(feature = "test-support")]
        #[test]
        fn evaluate_checkout_finalization_mints_evidence_on_full_agreement() {
            let Some((allocator, leases_dir)) = real_lease_for_eval_test("eval-happy-path") else {
                eprintln!("[checkout eval test] SKIP: no usable /etc/subuid|subgid range");
                return;
            };
            let mut lease = allocator.lease().unwrap();
            let mut session = CheckoutPreparationSession::new();
            session
                .bind_preparation(&mut lease, "c5".to_string(), (99, 99), (100, 100))
                .expect("bind_preparation must succeed");
            let expected = ExpectedGitCommitId::new(sha1_oid(0xa7), GitObjectFormat::Sha1).unwrap();
            let tree = sha1_oid(0xa8);
            let mut good_exit_good_line =
                outcome(format!("{} {tree}\n", expected.as_str()).as_bytes(), b"");
            good_exit_good_line.exit = Some(0);
            let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
                "c5".to_string(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: (99, 99),
                },
                CgroupQuiescenceEvidence::assert_for_tests((100, 100)),
            );
            let finalization: RuntimeFinalization<Result<RunscOutcome, RunFailure>> =
                RuntimeFinalization::Finalized(FinalizedRun {
                    primary: Ok(good_exit_good_line),
                    evidence,
                });
            let ws = temp_dir_for("happy-path-ws");
            std::fs::create_dir_all(ws.join(".git")).unwrap();
            std::fs::write(ws.join(".git/HEAD"), format!("{}\n", expected.as_str())).unwrap();
            std::fs::write(ws.join("Cargo.lock"), b"# fake lockfile content\n").unwrap();
            let result = evaluate_checkout_finalization(
                finalization,
                &mut lease,
                &mut session,
                1 << 20,
                &expected,
                &ws,
            );
            let prepared_evidence =
                result.expect("full agreement must mint PreparedCheckoutEvidence");
            assert_eq!(prepared_evidence.commit_hex(), expected.as_str());
            assert_eq!(prepared_evidence.tree_oid(), tree);
            let mut hasher = Sha256::new();
            hasher.update(b"# fake lockfile content\n");
            let expected_hex = hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            assert_eq!(prepared_evidence.cargo_lock_sha256_hex(), expected_hex);
            session
                .release_prepared(lease)
                .expect("session must have reached Prepared on the happy path");
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&leases_dir);
        }

        #[cfg(feature = "test-support")]
        #[test]
        fn evaluate_checkout_finalization_is_unreleasable_when_confirm_prepared_disagrees() {
            let Some((allocator, leases_dir)) = real_lease_for_eval_test("eval-confirm-mismatch")
            else {
                eprintln!("[checkout eval test] SKIP: no usable /etc/subuid|subgid range");
                return;
            };
            let mut lease = allocator.lease().unwrap();
            let mut session = CheckoutPreparationSession::new();
            session
                .bind_preparation(&mut lease, "c6".to_string(), (111, 111), (122, 122))
                .expect("bind_preparation must succeed");
            let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
                "c6".to_string(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: (111, 111),
                },
                CgroupQuiescenceEvidence::assert_for_tests((999, 999)),
            );
            let finalization: RuntimeFinalization<Result<RunscOutcome, RunFailure>> =
                RuntimeFinalization::Finalized(FinalizedRun {
                    primary: Ok(outcome(b"whatever", b"")),
                    evidence,
                });
            let ws = temp_dir_for("confirm-mismatch-ws");
            let expected = ExpectedGitCommitId::new(sha1_oid(0xa9), GitObjectFormat::Sha1).unwrap();
            let result = evaluate_checkout_finalization(
                finalization,
                &mut lease,
                &mut session,
                1 << 20,
                &expected,
                &ws,
            );
            match result {
                Err(CheckoutPreparationError::Unreleasable { usage, .. }) => {
                    assert!(
                        usage.is_some(),
                        "a post-spawn Unreleasable must carry measured usage"
                    );
                }
                other => panic!("expected Unreleasable, got {other:?}"),
            }
            assert!(session.is_unreleasable());
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&leases_dir);
        }
    }
}

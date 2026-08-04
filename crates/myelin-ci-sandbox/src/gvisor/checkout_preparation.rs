//! Hop B of a checkout: materializing the prefetched pack into the leased workspace inside a
//! dedicated preparation container, and verifying the result without following a symlink.

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
use crate::workspace_manager::{
    ManagedWorkspace, WorkspaceManager,
};
use crate::workspace_intent::ExpectedGitCommitId;
use crate::{
    CheckoutAuthorizationScope, EgressPolicy,
    LaunchPermit, PhaseAuthorization, ResourceLimits, ResourceUsage,
    RunTokenCredential, SandboxOutputSink,
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

/// CT-007 slice 5b.2: the checkout-specific analogue of [`GitWireSpec`] — carries only what the
/// checkout-preparation container needs: the exact commit to check out, Hop A's already-fetched
/// pack, and execution limits. Deliberately NO `run_token`/`meter_to`/`idem_token` (nothing here is
/// billed against job accounting on its own — see [`run_checkout_preparation`]'s doc) and NO
/// `OciWorkspaceMount`/`UserNamespaceConfig` (Sol's round-2 review: those are derived by the
/// executor directly from the REAL `ManagedWorkspace`/`UserNamespaceLease` being transitioned, never
/// from caller-supplied values that could silently name a different workspace/identity than the
/// capabilities actually in hand).
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct CheckoutPreparationSpec {
    pub(super) expected_commit: ExpectedGitCommitId,
    pack: PrefetchedCheckoutPack,
    limits: ResourceLimits,
}

impl CheckoutPreparationSpec {
    /// Fallible (Sol's review): bypassing `JobSpec` for this path also bypassed its mandatory
    /// `pids_max`/`timeout_secs` validation — this constructor enforces the SAME
    /// [`crate::validate_execution_limits`] check `JobSpec::new` would have, rather than silently
    /// accepting a zero fork-bomb ceiling or an infinite timeout.
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

/// The final, externally-meaningful result of a successful CT-007 checkout-preparation run — minted
/// ONLY once the preparation runtime's teardown was independently proven (`confirm_prepared`
/// succeeded, so the session is durably `Prepared`) AND the checkout itself was independently
/// verified (the in-guest `rev-parse`/`diff-index --quiet` confirmation line, AND the host's own
/// untrusted-fd `.git/HEAD` re-read) to be EXACTLY the expected commit — never merely "the container
/// exited 0." Fields are private; the only production constructor is [`run_checkout_preparation`].
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct PreparedCheckoutEvidence {
    commit_hex: String,
    tree_oid: String,
    /// Host-computed SHA-256 hex digest of the checked-out workspace's `Cargo.lock` (ledger 12's
    /// locked slice-5b contract) — slice 6's cargo-vendor EROFS asset is keyed off this.
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

    /// CT-007 slice 5b.3-6c/6e.2: a test-only constructor so `into_prepared_for_tests` (`#[cfg(test)]`)
    /// and the deterministic substituted Hop B seam (`test-support`) can supply a prepared capsule's
    /// evidence deterministically. Gated `#[cfg(any(test, feature = "test-support"))]` — absent from
    /// every ORDINARY (non-`test-support`) build, so `run_checkout_preparation` remains the only
    /// production constructor (the `test-support` feature is a dev-only build flag, never selected by
    /// any production composition root — pinned by the substrate's own production-zero source pins).
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

/// Every way CT-007 slice 5b.2's checkout-preparation runtime can fail to produce a
/// [`PreparedCheckoutEvidence`] (Sol's round-1 review, points 4/5). Distinguishes disposition along
/// the same axis [`RunFailure`] already established for the billed paths: `Refused` is genuinely
/// free (nothing ever spawned); every other variant carries the REAL measured usage a 5b.3 caller
/// must still fold into the parent attempt's aggregate settlement, never treat as free.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum CheckoutPreparationError {
    /// Refused before the preparation runtime itself ever spawned (bad transport/spec, or
    /// `bind_preparation` failed on a caller-fixable ground) — the lease/session are untouched, and
    /// there is no usage to account. Hop A's own failures are NOT necessarily free: once its
    /// advertisement run has executed, a Hop A failure surfaces as [`CheckoutTransportError`]
    /// (5b.3-3), which carries the real usage already measured — never collapsed into this `Refused`
    /// variant, which would silently discard it.
    Refused(String),
    /// `bind_preparation`/`confirm_prepared` poisoned the lease/session on a non-caller-fixable
    /// ground. `usage` is `None` iff this happened before any spawn attempt (a `bind_preparation`
    /// poisoning) — `Some` iff it happened after (a `confirm_prepared` poisoning, which can only
    /// occur after the container actually ran).
    Unreleasable {
        message: String,
        usage: Option<ResourceUsage>,
    },
    /// The preparation container ran, but [`finalize_runtime`] could not independently prove its
    /// teardown — `confirm_prepared` was never attempted (there is no valid proof to mint), so the
    /// session is STILL `PreparationBound`, forcing permanent quarantine on reconciliation, exactly
    /// like the non-checkout workload path.
    TeardownUnproven {
        message: String,
        usage: ResourceUsage,
    },
    /// Teardown was independently proven (`confirm_prepared` succeeded — the session is durably
    /// `Prepared`) but the attempt cannot continue. `disposition` distinguishes a terminal checkout
    /// failure/timeout, retryable infrastructure, or an invariant requiring reconciliation without
    /// parsing `message`. For terminal/retryable outcomes the workspace is provably garbage but the
    /// lease is fine, so the caller may delete the workspace and release the prepared session.
    /// `ReconciliationRequired` remains fail-closed: it must not be silently released or settled.
    RejectedAfterQuiescence {
        message: String,
        usage: ResourceUsage,
        disposition: PreparationAttemptDisposition,
    },
}

impl CheckoutPreparationError {
    /// The machine-readable outcome for Hop B. Diagnostic strings remain diagnostics only.
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

fn checkout_materialization_timed_out(
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

/// Classify a Hop B run failure only after runtime teardown was independently proven and the
/// preparation session reached `Prepared`. The immediate launch permit makes
/// `CommitOutcomeUnknown` unreachable in today's production path, but matching every variant here
/// keeps a future invariant violation fail-closed exactly like Hop A's [`map_hop_run_failure`].
fn map_checkout_materialization_run_failure(failure: RunFailure) -> CheckoutPreparationError {
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

/// The FIXED checkout-preparation guest script (CT-007 slice 5b.2; Sol's round-2 review), invoked as
/// `sh -c CHECKOUT_PREPARATION_SCRIPT sh <oid> <object-format> <shallow: 0|1>` — the dynamic values
/// reach the guest ONLY as positional parameters, never string-interpolated into the script text
/// itself (the same discipline [`RECEIVE_PACK_INGEST_SCRIPT`] already established, generalized to a
/// script that actually takes arguments).
///
/// - `set -eu` + `umask 077`: abort on any error/unset var; every file this script creates is
///   owner-only.
/// - Refuses if `/workspace` is not already empty (defense in depth — `WorkspaceManager` is
///   expected to always hand over a fresh subvolume, but this script never trusts that silently).
/// - `git init` with `--object-format` for the wanted hash width, and `core.hooksPath=/dev/null` +
///   an empty `--template=` — a `checkout` can invoke `post-checkout`; nothing here trusts a
///   default template's hook samples.
/// - Seeds `.git/shallow` with the wanted oid iff Hop A's fetch reported it as the shallow boundary.
/// - `git index-pack --stdin --strict`, never `--fix-thin` (a fresh, empty repo has no external
///   bases to repair a thin pack with) and never any `thin-pack`/side-band capability was requested.
/// - Compares command-substitution results with `test "$(...)" = "..."` (never trusts a bare exit
///   code alone) both for object presence-before-checkout and for HEAD after a detached checkout.
/// - Refuses (ledger 12's locked slice-5b contract) if the wanted commit's tree contains any
///   gitlink (mode `160000`) entry — this transport never fetches submodule repositories, so a
///   superproject with unpopulated submodules would silently build wrong; checked via
///   `git ls-tree -r` BEFORE checkout, so nothing is ever written for a shape this transport can't
///   honestly support.
/// - Refreshes the index and requires `diff-index --quiet` against the wanted commit's tree (the
///   worktree must be byte-identical to what the commit records, not merely "some file is there").
/// - Emits exactly one final line, `<commit-oid> <tree-oid>\n`, under a tiny stdout footprint — the
///   ONLY thing `run_checkout_preparation` reads back besides the exit code.
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

/// The gitlink-detection portion of [`CHECKOUT_PREPARATION_SCRIPT`], duplicated verbatim (minus the
/// surrounding checkout machinery) so it is directly testable via real host `git`+`sh` — no gVisor
/// needed, since this only exercises shell/git logic, never sandboxing. KEEP IN SYNC with the same
/// lines in `CHECKOUT_PREPARATION_SCRIPT` (invoked the same way: `sh -c ... sh <oid>`); emits `ok`
/// on success so a test can also confirm the positive (no-gitlinks) path actually ran to completion.
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

/// Parse the checkout script's one confirmation line (`<commit-oid> <tree-oid>\n`) — refuses
/// anything else (extra fields, a mismatched commit, a malformed tree oid) rather than trusting the
/// guest's exit code alone.
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

/// Open a REGULAR file, by name, relative to `dir_fd`, with `O_NOFOLLOW` — a symlinked name fails to
/// open at all (`ELOOP`), and anything opened that is NOT a regular file (a FIFO/device/socket a
/// guest process could have created in its own writable workspace) is refused after the open.
#[allow(dead_code)]
fn open_regular_file_no_follow(dir_fd: RawFd, name: &CString) -> io::Result<std::fs::File> {
    // SAFETY: `dir_fd` is a valid, open directory file descriptor for the duration of this call;
    // `name` is a NUL-terminated component name. `O_NONBLOCK` is load-bearing (Sol's review): a
    // guest process (which fully owns its own writable workspace) could plant a FIFO named `HEAD`
    // — a plain `O_RDONLY` open on a FIFO with no writer BLOCKS the caller indefinitely, before the
    // `is_file()` check below is ever reached. `O_NONBLOCK` makes `openat` return immediately
    // instead; a regular file's read behavior is unaffected by the flag.
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
    // SAFETY: `fd` was just returned by a successful `openat` above and is not owned elsewhere.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a regular file",
        ));
    }
    Ok(file)
}

/// Host-side, FD-safe re-verification of a checked-out workspace's HEAD (Sol's round-2 review point
/// 3): the guest's own `rev-parse`/`diff-index` claims are NOT independently trusted alone. Walks
/// `<workspace>/.git` via `openat(O_NOFOLLOW)` at every component (never a path-based `std::fs::read`
/// that could follow a guest-planted symlink), and requires `.git` to be a real directory and
/// `.git/HEAD` a bounded regular file containing EXACTLY `<expected-oid>\n` — HEAD must be detached
/// (the checkout script never leaves a symbolic ref), so no symbolic-ref resolution is implemented or
/// needed. Never invokes host `git` over the guest-written repository (the guest already fully owns
/// interpreting its own object store; this check only re-reads one small, bounded file by exact
/// content).
#[allow(dead_code)]
fn verify_workspace_head_no_follow(
    workspace_host_path: &Path,
    expected: &ExpectedGitCommitId,
) -> Result<(), String> {
    let path_c = CString::new(workspace_host_path.as_os_str().as_encoded_bytes())
        .map_err(|e| format!("workspace path contains an interior NUL: {e}"))?;
    // SAFETY: standard POSIX flags on a NUL-free path; the workspace directory is host-provisioned
    // (`WorkspaceManager` already created it) and this open follows no untrusted symlink.
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
    // SAFETY: `workspace_fd` was just returned by a successful `open` above and is not owned
    // elsewhere.
    let workspace_fd = unsafe { OwnedFd::from_raw_fd(workspace_fd) };
    let git_name = CString::new(".git").expect("no interior NUL");
    let git_fd = crate::dirlock::open_dir_component_no_follow(workspace_fd.as_raw_fd(), &git_name)
        .map_err(|e| format!(".git is not a real directory (or is a symlink): {e}"))?;
    let head_name = CString::new("HEAD").expect("no interior NUL");
    let mut head_file = open_regular_file_no_follow(git_fd.as_raw_fd(), &head_name)
        .map_err(|e| format!(".git/HEAD is not a real regular file (or is a symlink): {e}"))?;
    // Bound the ACTUAL READ (Sol's review), not merely a preceding `metadata().len()` check — a
    // stat-then-read is a check-then-act gap in general, and the enforcement must live in the read
    // itself. A detached HEAD file is `<40-or-64 hex>\n` -- at most 65 bytes; reading one MORE than
    // the generous 128-byte bound below is enough to detect "there is more data than expected"
    // directly from the read, never from an unbounded `read_to_string`.
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

/// The bound on `Cargo.lock`'s hashed size — generous for even a very large Cargo workspace lockfile,
/// never unbounded.
const CARGO_LOCK_HASH_BOUND: u64 = 16 * 1024 * 1024;

/// Host-side, FD-safe hash of the checked-out workspace's `Cargo.lock` (ledger 12's locked slice-5b
/// contract: "hashes the materialized `Cargo.lock`" so [`PreparedCheckoutEvidence`] can carry the
/// digest slice 6's cargo-vendor EROFS asset keys off of). Opened relative to the workspace
/// directory with `O_NOFOLLOW` (mirrors [`verify_workspace_head_no_follow`]'s own discipline — never
/// a guest-planted symlink), and required to be a real regular file under
/// [`CARGO_LOCK_HASH_BOUND`] bytes. Host-COMPUTED, never a guest-reported hash: this digest becomes
/// a downstream cache/asset key, so it must be independently authoritative, the same reasoning that
/// makes `HEAD` itself host-re-read rather than merely trusted from the guest's own confirmation
/// line. Absence, a non-regular-file, or exceeding the bound are all refusals — this checkout
/// transport exists for Cargo-workspace CI jobs, so a missing `Cargo.lock` is a real failure, never
/// silently "no digest."
fn hash_workspace_cargo_lock_no_follow(workspace_host_path: &Path) -> Result<String, String> {
    let path_c = CString::new(workspace_host_path.as_os_str().as_encoded_bytes())
        .map_err(|e| format!("workspace path contains an interior NUL: {e}"))?;
    // SAFETY: standard POSIX flags on a NUL-free path; the workspace directory is host-provisioned
    // and this open follows no untrusted symlink.
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
    // SAFETY: `workspace_fd` was just returned by a successful `open` above and is not owned
    // elsewhere.
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

/// A per-thread, effective-only `CAP_DAC_READ_SEARCH` scope. The capability must already be dormant
/// in the permitted set (the shape [`prepare_checkout_host_verification_capability`] establishes
/// before application threads exist). A pre-existing effective capability is refused: silently
/// accepting that state would turn a process-wide deployment mistake into an apparently-scoped
/// verification.
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
            // Continuing the runner after failing to withdraw a host-wide read bypass would make
            // the advertised scope false. Abort fail-closed; systemd's restart policy rebuilds the
            // process from the prepared initial capability state.
            std::process::abort();
        }
    }
}

/// Perform BOTH authoritative host reads inside one narrowly bounded capability scope. In ordinary
/// unprivileged unit tests (where the fixture is runner-owned and the capability is wholly absent),
/// perform the same no-follow reads under normal DAC. Production explicit-userns activation calls
/// [`prepare_checkout_host_verification_capability`] before constructing its runtime, so a real
/// subuid-owned checkout always takes the scoped branch and startup fails before claims if the
/// capability was not supplied.
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

/// The measured [`ResourceUsage`] of one `RunscOutcome` — the SAME derivation [`build_result`] uses,
/// extracted so the checkout-preparation path (which has no real `JobSpec` to hand `build_result`,
/// by design — see [`run_checkout_preparation`]'s doc) can compute it from a bare `mem_bytes` value.
#[allow(dead_code)]
fn usage_from_runsc_outcome(mem_bytes: u64, o: &RunscOutcome) -> ResourceUsage {
    let wall_secs_ceil = o.wall.as_secs() + u64::from(o.wall.subsec_nanos() > 0);
    let cpu_seconds = o.cpu_seconds.filter(|c| *c > 0).unwrap_or(wall_secs_ceil);
    ResourceUsage {
        cpu_seconds,
        mem_byte_seconds: mem_bytes.saturating_mul(wall_secs_ceil),
    }
}

/// CT-007 slice 5b.2, Hop B: run the checkout-preparation runtime for an ALREADY-ACQUIRED
/// [`ManagedWorkspace`] + [`UserNamespaceLease`] + [`CheckoutPreparationSession`] (slice 5b.1's
/// types) — the resource-reservation choreography (acquiring those in the first place, and the real
/// workload's own `LaunchPermit` afterward) is slice 5b.3's job, not this function's.
///
/// This IS a real, measured sandbox execution: it performs no `reserve`/`settle` of its own (there
/// is no per-checkout job to reserve against), but its usage is charged through the PARENT
/// ATTEMPT's aggregate settlement in slice 5b.3 — see [`CheckoutPreparationError`]'s `usage` fields,
/// which carry the REAL measured cost on every post-spawn failure, never silently free.
///
/// Uses an internally-minted [`LaunchPermit::immediate`] (never one supplied by the caller) so the
/// preparation container still runs through the SAME mechanical launch-gate + watchdog every other
/// `runsc` spawn does, without performing the workload's own durable launch CAS (Sol's round-1
/// review: `launch_permit: None` would have skipped the gate/watchdog ENTIRELY, not merely the CAS).
///
/// Ordering (Sol's round-1 review, the load-bearing property): `session.confirm_prepared` is called
/// the moment teardown is independently proven, REGARDLESS of whether the checkout itself succeeded
/// — an ordinary corrupt pack or a wrong checked-out tree must never force permanent quarantine of a
/// lease whose runtime genuinely tore down cleanly. Only AFTER that durable transition does this
/// function check the command's exit status, the guest's own confirmation line, and the host's
/// independent `.git/HEAD` re-read; any of those failing returns
/// [`CheckoutPreparationError::RejectedAfterQuiescence`] with the session left `Prepared` — the
/// slice 5b.3 caller must then delete the workspace and call `session.release_prepared`, never
/// quarantine the identity.
/// **The LEGACY (V1) Hop B entry point.** Signature and behaviour exactly as shipped: the
/// preparation container runs under an internally-minted immediate permit, because a V1 preparation
/// has no durable phase gate to consult.
#[allow(dead_code)]
pub(crate) fn run_checkout_preparation(
    lease: &mut UserNamespaceLease,
    session: &mut CheckoutPreparationSession,
    workspace: &ManagedWorkspace,
    spec: CheckoutPreparationSpec,
) -> Result<PreparedCheckoutEvidence, CheckoutPreparationError> {
    // Legacy Hop B keeps its historical behaviour: never cancelled, no incremental output sink.
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

/// The ONE cleanup a checkout capsule's session disposition permits, as a PURE value (CT-007 slice
/// 5b.3-6a, Sol's r1 blocker 6) so the state→action mapping is unit-testable WITHOUT a real
/// workspace/lease/`CAP_SYS_ADMIN`. [`AcquiredCheckoutRuntime::dispose_checkout_runtime`] executes
/// exactly this plan; a regression that swapped e.g. the `NeverBound` and `Prepared` release methods
/// changes this mapping and fails the pure pin, rather than only surfacing as a production
/// panic/allocator poison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(super) enum CheckoutCleanupPlan {
    /// The lease is provably still `Allocated` — delete the workspace, then `release_unused`.
    DeleteWorkspaceThenReleaseUnused,
    /// Teardown proven, workload never bound — delete the workspace, then (only on proven disk
    /// absence) `release_prepared`; otherwise quarantine the lease.
    DeleteWorkspaceThenReleasePrepared,
    /// Teardown unproven or the lease already poisoned — quarantine BOTH; never release, never delete.
    QuarantineBoth,
    /// The workload durably bound; the existing finalization path owns the resources. Disposal here
    /// is structurally impossible — abandon BOTH and surface an invariant violation.
    AbandonBoth,
}

/// The pure disposition→plan mapping (see [`CheckoutCleanupPlan`]).
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

/// The FOUR primitive cleanup operations a [`CheckoutCleanupPlan`] executes against the shared
/// workspace+lease+session, behind an injectable seam (CT-007 slice 5b.3-6a, Sol's r2 blocker 3) so
/// that an ALWAYS-RUN unit test with a recording fake can prove each plan invokes EXACTLY the right
/// operation sequence — no Btrfs/`CAP_SYS_ADMIN` required. Swapping the `release_unused`/
/// `release_prepared` legs of the two delete-then-release plans changes the recorded trace and fails
/// that test, rather than only surfacing as a production panic/allocator poison. The REAL
/// implementation ([`RealCheckoutCleanupExecutor`]) performs the genuine deletes/releases against the
/// held resources; the privileged e2e matrix then proves those real ops release/quarantine durably.
#[allow(dead_code)]
pub(super) trait CheckoutCleanupExecutor {
    /// Delete the workspace; return whether disk absence is PROVEN (only then may the lease be
    /// released) plus any diagnostics.
    fn delete_workspace(&mut self) -> (bool, Vec<String>);
    /// Release a provably-`Allocated` lease (`release_unused`).
    fn release_unused(&mut self) -> Vec<String>;
    /// Release a provably-`Prepared` lease (`session.release_prepared`).
    fn release_prepared(&mut self) -> Vec<String>;
    /// Quarantine the workspace — never delete (drop it; its own `Drop` poisons the manager).
    fn quarantine_workspace(&mut self);
    /// Quarantine the lease — never release (drop it; its own `Drop` quarantines the slot).
    fn quarantine_lease(&mut self);
}

/// Execute one [`CheckoutCleanupPlan`] through the injected executor, returning accumulated
/// diagnostics. This is the SINGLE place the plan→operation-sequence mapping lives, so the always-run
/// trace test and the real disposal share exactly one implementation.
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
                // Disk absence NOT proven — the lease must NOT be released; quarantine it.
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
                "dispose_checkout_runtime reached a WorkloadBound capsule — this should be \
                 structurally impossible (a bound workload's workspace and lease are owned by the \
                 existing finalization/settlement path); both are abandoned (quarantined) rather \
                 than acted on"
                    .to_string(),
            );
        }
    }
    diagnostics
}

/// The REAL [`CheckoutCleanupExecutor`]: it OWNS the capsule's disassembled resources and performs the
/// genuine delete/release/quarantine. Each op `take`s its resource out of an `Option` exactly once —
/// the plan sequences guarantee no op is invoked twice or on a missing resource.
#[allow(dead_code)]
pub(super) struct RealCheckoutCleanupExecutor<'a> {
    pub(super) workspace: Option<ManagedWorkspace>,
    pub(super) lease: Option<UserNamespaceLease>,
    // Named DISTINCTLY from the capsule's `session` field on purpose: the AST inseparability guard
    // confines every access of a capsule inner field (`session` included) to an allowlist of capsule
    // methods, so an unrelated struct reusing that field name would otherwise force this executor's
    // methods onto that allowlist and weaken the guard.
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
        // Drop (never delete): `ManagedWorkspace::drop` poisons the workspace manager.
        drop(self.workspace.take());
    }

    fn quarantine_lease(&mut self) {
        // Drop (never release): `UserNamespaceLease::drop` quarantines the slot.
        drop(self.lease.take());
    }
}

/// CT-007 slice 5b.3-6c: the OWNED typed outcome of the closed capsule op
/// [`checkout_runtime::PreparedCheckoutRuntime::run_retained_workload`]. The op returns ONLY this —
/// never a borrow of the capsule's retained `OciConfig`/lease/session/workspace, never a
/// `RuntimePreparation` — so nothing that could reconstitute or cross-wire the 6a capsule ever
/// escapes the call. Defined here in the parent module (the capsule submodule's own shape is
/// closed-world audited and may not add types), and every disposal branch already disposed the
/// capsule along its exact session disposition before returning.
#[allow(dead_code)]
pub(crate) enum RetainedWorkloadOutcome {
    /// The workload's `run` produced a settled result and the capsule's workspace/lease were settled
    /// through the audited `settle_enabled_finalization` tail. `Ok` carries a real [`ContainerRun`]
    /// (the continuation does the live-map insert + completion settle + `SandboxLaunch`); `Err` is a
    /// post-settle [`RunFailure`] the continuation routes through the existing workload machinery.
    Ran(Result<ContainerRun, RunFailure>),
    /// `run` returned a pre-finalization [`RunFailure`] (bundle staging, cgroup, the durable workload
    /// bind, or a spawn failure before a trustworthy result). The capsule was disposed along its
    /// session disposition; the `RunFailure` phase tells the continuation whether this is a pre-bind
    /// requeue/exhaust or a post-bind reporter retryable.
    RunFailed {
        failure: RunFailure,
        disposal_diagnostics: Vec<String>,
    },
    /// The materialization-phase completion op failed structurally; the capsule was disposed
    /// (`Prepared → release_prepared`).
    PhaseAuthorityFailed {
        error: crate::checkout_orchestration::AttemptAuthorityError,
        disposal_diagnostics: Vec<String>,
    },
    /// The preparation lease renewal was refused (the generation is no longer ours); the capsule was
    /// disposed (`Prepared → release_prepared`).
    LeaseLost {
        lost: crate::runner::PreparationLeaseLost,
        disposal_diagnostics: Vec<String>,
    },
    /// The workload launch permit was refused before execution; the capsule was disposed
    /// (`Prepared → release_prepared`).
    PermitRefused {
        message: String,
        disposal_diagnostics: Vec<String>,
    },
}

/// Hop B's ENTIRE pre-spawn V2 authorization decision, extracted so it is unit-testable without a
/// real lease/session/workspace/`runsc` (the same convention [`evaluate_checkout_finalization`]
/// follows). Consumes the authorization against the capsule's FULL derived scope (Sol's r1 blocker 1),
/// not just the commit.
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
    // CT-007 slice 5b.3-6c: the SAME cancellation object threaded through Hop A / Hop B / workload —
    // Hop B no longer hardcodes `NEVER_CANCELLED`. And the SAME bounded/redacted output sink the
    // workload uses, so preparation diagnostics stream durably rather than surfacing only in errors.
    cancellation: &AtomicBool,
    output: Option<Arc<dyn SandboxOutputSink>>,
) -> Result<PreparedCheckoutEvidence, CheckoutPreparationError> {
    let bin = runsc_bin();
    let root_abs = verified_gvisor_git_rootfs().map_err(CheckoutPreparationError::Refused)?;
    let userns = lease.config();
    let workspace_mount = OciWorkspaceMount::from_managed_workspace(workspace);

    let profile = HardeningProfile::for_execution(&spec.limits, &EgressPolicy::deny_all());
    // Sol's review: bypassing `JobSpec`/`launch_git_command`'s own `.assert_enforced()` call for
    // this path would silently skip the fail-closed check that the derived profile is ACTUALLY
    // fully in force (egress default-deny, read-only root, caps dropped, etc.) before anything
    // spawns -- assert it here too, exactly like every other production launch path does.
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

    // Revalidate the runsc-root identity live, immediately before it is baked into
    // `PreparedRuntimeMode` — same pattern as the ordinary workload path's `RuntimeBinding::Enabled`
    // construction (`launch_with`), re-revalidated again below, right at the actual bind boundary.
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

    // CT-003b (SI-017): the out-of-band memory cgroup, established BEFORE anything durably commits.
    let cgroup = match MemoryCgroup::create(spec.limits.mem_bytes, spec.limits.cpu_millis) {
        Ok(cgroup) => cgroup,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&bundle_dir);
            return Err(CheckoutPreparationError::Refused(e));
        }
    };
    let container_id = format!("myelin-checkout-{}-{}", std::process::id(), unique_suffix());

    // Durably bind `session`/`lease` to the preparation runtime BEFORE ever calling
    // `run_and_capture` -- immediately after the cgroup exists (so `cgroup.identity()` is
    // available), while nothing has spawned yet. Sol's review (a real bug, not merely a doc
    // mismatch): the EARLIER revalidation above only seeds `prepared_mode`/`mode` -- it must NOT
    // be reused as the value actually bound. Re-revalidate AGAIN, live, right at this exact
    // boundary (mirrors `bind_enabled_lease_given`'s established two-check pattern precisely: the
    // earlier read is what THIS fresh one is compared against, never a substitute for it), and
    // bind the FRESH value -- a drift between the two refuses here, with the lease/session still
    // completely untouched.
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

/// The decision logic behind [`run_checkout_preparation`]'s ENTIRE post-spawn disposition (Sol's
/// review: extracted so it is unit-testable against synthetic [`RuntimeFinalization`]/
/// [`RuntimeQuiescenceEvidence`] values, without any real `runsc` spawn at all). Implements the
/// exact ordering Sol's round-1 review specified as load-bearing:
///
/// 1. If teardown could not be independently proven (`RuntimeFinalization::Failed`),
///    `confirm_prepared` is NEVER attempted — the session stays durably `PreparationBound`,
///    forcing permanent quarantine on reconciliation (`TeardownUnproven`).
/// 2. Otherwise, mint the [`PreparationQuiescenceProof`] and call `session.confirm_prepared`
///    UNCONDITIONALLY — regardless of the guest's exit code or confirmation line. An ordinary
///    corrupt pack or wrong checkout must never force permanent quarantine of a lease whose
///    runtime genuinely tore down cleanly.
/// 3. ONLY once `session` is durably `Prepared` does this check the guest's exit status, its
///    confirmation line, and the host's independent `.git/HEAD` re-read. Any disagreement here is
///    `RejectedAfterQuiescence` — the session is left `Prepared` (the caller must delete the
///    workspace and call `session.release_prepared`, never quarantine the identity).
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

    // Teardown is independently proven AND durably confirmed (`session` is now `Prepared`). ONLY
    // now do we look at whether the checkout itself was actually right.
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
    // Sol's review: a truncated confirmation line or a stream/output error must ALSO be treated as
    // a semantic failure (never silently trusted as if the (possibly incomplete) captured stdout
    // were the guest's real, complete output) — checked here, AFTER `confirm_prepared` already ran.
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

/// A short, human-readable description of a checkout run's primary disposition, for a
/// `TeardownUnproven` diagnostic message — never the sole basis for any decision.
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

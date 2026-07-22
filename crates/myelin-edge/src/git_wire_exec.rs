//! # The PRODUCTION git `WireExecutor` — sandboxed canonical `git` at the serving tier (CT-006b / GT-006)
//!
//! [`myelin_git::core::WireExecutor`] is the no-host-exec port the [`myelin_git::core::ShellGitCore`]
//! wire backend routes every `upload-pack` / `receive-pack` / `ls-refs` / maintenance invocation
//! through (the git-shaped analogue of `ToolHands::exec` for the agent fabric). The git crate ships
//! only TEST executors; the PRODUCTION one lives HERE, in the serving tier, NOT in `myelin-git/src/`,
//! so the `no-host-exec` lint stays green over the git crate (the executor owns the sandboxed launch).
//!
//! ## What it does
//! [`GitWireExecutor`] maps a [`WireInvocation`] `{ repo, argv, stdin }` to a
//! [`myelin_ci_sandbox::GitWireSpec`] and runs it through
//! [`myelin_ci_sandbox::gvisor::GvisorBackend::launch_git_wire`] — canonical `git <argv> /repo` inside
//! the PROVEN hardened gVisor sandbox (CT-002/003/006a: egress default-deny + no-netns, read-only root
//! + tmpfs scratch, all caps dropped, no-new-privileges, seccomp, non-root uid, mem/pids/disk bounded,
//!
//! whole-container kill + cleanup, bounded capture). The bare repo is bound **READ-ONLY** at `/repo`;
//! the locator is resolver-validated (the GT-001 cross-tenant boundary, replicated + drift-pinned) and
//! symlink-confined before any mount. **No host-exec fingerprint here** — the edge NEVER calls
//! `Command`; it delegates the launch to `launch_git_wire`.
//!
//! ## Exit / timeout fidelity (never a silent empty stdout)
//! A non-zero `git` exit or a wall-clock timeout is mapped to [`GitCoreError::Wire`] — the seam never
//! returns an empty `WireOutput` for a failed run. A clean exit (0, not timed out) returns the captured
//! stdout (the ref advertisement / packfile bytes).
//!
//! ## CT-006c floor (stated, not built here)
//! - **receive-pack / PUSH**: the writable-quarantine-under-rootless-runsc intake + the in-process
//!   policy / `git fsck` / one-tx ref-CAS + outbox is CT-006c. This executor passes NO quarantine
//!   (correct for `upload-pack`); a `receive-pack` serve would need the quarantine wiring CT-006c adds.
//! - **the HTTP smart-transport server binary/listener** + the external-oracle `git clone`/`fetch`/
//!   `push` are CT-006c. CT-006b proves the path through the [`myelin_git::core::GitCore`] seam.

use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    GitWireSpec, IdemToken, MeterTarget, ResourceLimits, RunTokenCredential, RunnerHooks,
    SandboxBackend, SandboxLaunch,
};
use myelin_git::core::{GitCoreError, RoutedGitCore, WireExecutor, WireInvocation, WireOutput};
use myelin_git::gix_backend::{GixCore, RootedResolver};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Authenticated facts presented to the serving-tier credential issuer for one sandboxed Git wire
/// invocation. The issuer itself is expected to be request-bound to the verified principal; no
/// bearer material or caller-selectable policy enters this carrier.
pub struct GitWireCredentialRequest<'a> {
    pub tenant: &'a str,
    pub region: &'a str,
    pub repo: &'a str,
    pub operation: &'a str,
    pub invocation_id: &'a str,
    pub ttl_secs: u64,
}

/// Final-boundary minting port for sandboxed Git wire work. A production implementation calls the
/// Identity token authority; the executor has no fallback that fabricates credentials.
pub trait GitWireCredentialIssuer: Send + Sync {
    fn mint(&self, request: &GitWireCredentialRequest<'_>) -> Result<RunTokenCredential, String>;
}

#[derive(Debug)]
struct UnavailableGitWireCredentialIssuer;

impl GitWireCredentialIssuer for UnavailableGitWireCredentialIssuer {
    fn mint(&self, _request: &GitWireCredentialRequest<'_>) -> Result<RunTokenCredential, String> {
        Err("no live Identity Git-wire credential issuer is configured".into())
    }
}

pub(crate) fn unavailable_git_wire_credential_issuer() -> Arc<dyn GitWireCredentialIssuer> {
    Arc::new(UnavailableGitWireCredentialIssuer)
}

/// Explicit deterministic issuer for tests/drills. It is absent from the default production graph;
/// production composition must bind Identity instead.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub struct TestGitWireCredentialIssuer;

#[cfg(any(test, feature = "test-support"))]
impl GitWireCredentialIssuer for TestGitWireCredentialIssuer {
    fn mint(&self, request: &GitWireCredentialRequest<'_>) -> Result<RunTokenCredential, String> {
        RunTokenCredential::new(
            format!("test-git-wire-bearer:{}", request.invocation_id),
            format!("test-git-wire-jti:{}", request.invocation_id),
            request.ttl_secs,
        )
        .map_err(|error| error.to_string())
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn test_git_wire_credential_issuer() -> Arc<dyn GitWireCredentialIssuer> {
    Arc::new(TestGitWireCredentialIssuer)
}

/// The production [`WireExecutor`]: every canonical-`git` wire invocation runs SANDBOXED through the
/// gVisor backend's [`GvisorBackend::launch_git_wire`]. Rooted at the on-disk dir holding
/// `<tenant>/<region>/<repo>.git` bare repos (the SAME root the read backend [`GixCore`] /
/// [`RootedResolver`] and the durable store resolve against). Holds the four-guarantee
/// [`RunnerHooks`] the launch drives (cost gate, attribution, isolation floor) — the wallet/identity/
/// KMS bodies are consumed contracts the composition root binds.
pub struct GitWireExecutor {
    backend: GvisorBackend,
    root: PathBuf,
    limits: ResourceLimits,
    hooks: RunnerHooks,
    credential_issuer: Arc<dyn GitWireCredentialIssuer>,
    shutdown: Arc<AtomicBool>,
    /// Monotone per-invocation sequence → a unique idempotency token per sandboxed launch (the runner
    /// dedups on it; one sandbox per wire op, never reused).
    seq: AtomicU64,
}

impl GitWireExecutor {
    /// Build the production executor over an on-disk git root, resource limits, and the four-guarantee
    /// hooks. The caller (the composition root / CT-006c server) supplies real hooks; nothing here
    /// fabricates a guarantee.
    pub fn new(
        root: impl Into<PathBuf>,
        limits: ResourceLimits,
        hooks: RunnerHooks,
        credential_issuer: Arc<dyn GitWireCredentialIssuer>,
    ) -> Self {
        Self {
            backend: GvisorBackend::new(),
            root: root.into(),
            limits,
            hooks,
            credential_issuer,
            shutdown: Arc::new(AtomicBool::new(false)),
            seq: AtomicU64::new(0),
        }
    }

    /// Bind the process-level shutdown signal used to cancel an active `runsc` wire container. The
    /// default stays never-cancelled for isolated tests and non-serving callers.
    pub fn with_shutdown_signal(mut self, shutdown: Arc<AtomicBool>) -> Self {
        self.shutdown = shutdown;
        self
    }

    /// Serving-tier default: sane resource bounds + the four-guarantee hooks wired to pass-through
    /// seams. Credential minting deliberately remains unavailable, so a wire operation fails closed
    /// until the composition root injects a live Identity issuer with [`Self::new`].
    pub fn serving_default(root: impl Into<PathBuf>) -> Self {
        Self::new(
            root,
            Self::default_limits(),
            Self::serving_hooks(),
            unavailable_git_wire_credential_issuer(),
        )
    }

    /// The serving-tier resource bounds for a wire op (every field non-zero: the `JobSpec` invariants
    /// require `pids_max > 0` and `timeout_secs > 0`).
    pub fn default_limits() -> ResourceLimits {
        ResourceLimits {
            cpu_millis: 2000,
            mem_bytes: 512 * 1024 * 1024,
            disk_bytes: 512 * 1024 * 1024,
            pids_max: 256,
            timeout_secs: 120,
        }
    }

    /// The four-guarantee hooks for the serving tier. The launch ALWAYS asserts the mandatory hardening
    /// profile is in force itself (`HardeningProfile::assert_enforced`), so `isolation_floor` here is a
    /// pass-through; reserve/settle/attribute are the seams the composition root binds to the live
    /// wallet (11.7) / token mint (4.7). Returning `Ok` here does NOT skip the floor — the floor is
    /// enforced unconditionally inside `launch_git_wire`.
    pub fn serving_hooks() -> RunnerHooks {
        RunnerHooks {
            reserve: Box::new(|m| Ok(myelin_ci_sandbox::ReserveHandle(m.reserve_id.clone()))),
            settle: Box::new(|_h, _u| Ok(())),
            attribute: Box::new(|_t| Ok(())),
            isolation_floor: Box::new(|_s| Ok(())),
        }
    }

    /// The git environment the sandboxed `upload-pack` needs: the git-core exec dir, a writable `$HOME`
    /// on the tmpfs, the system-config opt-out, and `safe.directory=*` (the RO repo is owned by the
    /// host user, not the in-guest uid 65534, so without this `git` refuses the "dubious ownership"
    /// repo). v0 stateless-rpc (no `GIT_PROTOCOL=version=2`) — the wire seam drives v0 advertise/serve.
    fn wire_env() -> Vec<String> {
        vec![
            "HOME=/tmp".to_string(),
            "GIT_EXEC_PATH=/usr/lib/git-core".to_string(),
            "GIT_CONFIG_NOSYSTEM=1".to_string(),
            "GIT_CONFIG_COUNT=1".to_string(),
            "GIT_CONFIG_KEY_0=safe.directory".to_string(),
            "GIT_CONFIG_VALUE_0=*".to_string(),
        ]
    }

    /// Mint the per-invocation credential and derive non-secret accounting/idempotency handles. A
    /// missing/refusing Identity issuer is a hard wire error before path resolution or sandbox spawn.
    fn next_tokens(
        &self,
        repo: &myelin_git::core::RepoLoc,
        operation: &str,
    ) -> Result<(RunTokenCredential, MeterTarget, IdemToken), GitCoreError> {
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let tag = format!("git-wire-{}-{n}", std::process::id());
        let credential = self
            .credential_issuer
            .mint(&GitWireCredentialRequest {
                tenant: &repo.tenant,
                region: &repo.region,
                repo: &repo.repo,
                operation,
                invocation_id: &tag,
                ttl_secs: 120,
            })
            .map_err(|error| {
                GitCoreError::Wire(format!("Git-wire credential mint refused: {error}"))
            })?;
        Ok((
            credential,
            MeterTarget {
                reserve_id: format!("{tag}-reserve"),
            },
            IdemToken(tag),
        ))
    }
}

impl GitWireExecutor {
    /// **Ingest a pushed packfile in the hardened sandbox (CT-006d).** Drives
    /// [`GvisorBackend::launch_git_receive_pack`]: the UNTRUSTED client pack is piped to the sandbox,
    /// `git index-pack --fix-thin` validates + resolves it against the RO `/repo` alternates inside the
    /// writable `/tmp` tmpfs quarantine (never the real repo), and the FULLY-RESOLVED objects are streamed
    /// back as a `git cat-file --batch` stream (`<oid> <type> <size>\n<payload>\n` repeated). A non-zero
    /// `index-pack` exit (corrupt/forged/incomplete pack) or a timeout is a HARD error — never a silent
    /// empty result. The repo stays READ-ONLY to the sandbox; the host parses + policies + migrates.
    pub fn ingest_pack(
        &self,
        repo: &myelin_git::core::RepoLoc,
        pack: Vec<u8>,
    ) -> Result<Vec<u8>, GitCoreError> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(GitCoreError::Wire(
                "sandboxed receive-pack ingest cancelled by process shutdown".into(),
            ));
        }
        let (rt, mt, it) = self.next_tokens(repo, "receive-pack-ingest")?;
        let spec = GitWireSpec::for_repo(
            &self.root,
            &repo.tenant,
            &repo.region,
            &repo.repo,
            Vec::new(), // ignored for receive-pack ingest (the fixed `sh -c` ingest script runs)
            pack,
            Self::wire_env(),
            None, // no host-bind quarantine — the quarantine is the in-guest /tmp tmpfs (rootless-safe)
            self.limits,
            rt,
            mt,
            it,
        )
        .map_err(|e| GitCoreError::Wire(e.to_string()))?;

        let SandboxLaunch { handle, result } = self
            .backend
            .launch_git_receive_pack_until_cancelled(&spec, &self.hooks, &self.shutdown)
            .map_err(|e| GitCoreError::Wire(e.to_string()))?;
        let _ = self.backend.kill(&handle);

        if self.shutdown.load(Ordering::Acquire) {
            return Err(GitCoreError::Wire(
                "sandboxed receive-pack ingest cancelled by process shutdown".into(),
            ));
        }
        if result.timed_out {
            return Err(GitCoreError::Wire(format!(
                "sandboxed receive-pack ingest timed out ({}s ceiling)",
                self.limits.timeout_secs
            )));
        }
        match result.exit_code {
            Some(0) => Ok(result.stdout),
            other => Err(GitCoreError::Wire(format!(
                "sandboxed receive-pack ingest (git index-pack) exited {other:?}: {}",
                String::from_utf8_lossy(&result.stderr)
            ))),
        }
    }
}

impl WireExecutor for GitWireExecutor {
    fn run(&self, inv: &WireInvocation) -> Result<WireOutput, GitCoreError> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(GitCoreError::Wire(format!(
                "sandboxed `git {}` cancelled by process shutdown",
                inv.argv.join(" ")
            )));
        }
        let operation = inv.argv.first().map(String::as_str).unwrap_or("git-wire");
        let (rt, mt, it) = self.next_tokens(&inv.repo, operation)?;
        // Map the WireInvocation locator → a resolver-validated, symlink-confined GitWireSpec. A
        // cross-tenant / `..` / separator locator is REFUSED here (the GT-001 boundary), before mount.
        // NO quarantine for upload-pack (read-only serve); the receive-pack writable quarantine is CT-006c.
        let spec = GitWireSpec::for_repo(
            &self.root,
            &inv.repo.tenant,
            &inv.repo.region,
            &inv.repo.repo,
            inv.argv.clone(),
            inv.stdin.clone(),
            Self::wire_env(),
            None,
            self.limits,
            rt,
            mt,
            it,
        )
        .map_err(|e| GitCoreError::Wire(e.to_string()))?;

        let SandboxLaunch { handle, result } = self
            .backend
            .launch_git_wire_until_cancelled(&spec, &self.hooks, &self.shutdown)
            .map_err(|e| GitCoreError::Wire(e.to_string()))?;

        // One sandbox per wire op — tear it down (idempotent; the guest has already exited).
        let _ = self.backend.kill(&handle);

        // Honor the exit/timeout: a timeout or a non-zero `git` exit is a HARD error, never a silent
        // empty stdout. stderr is folded into the error message (capped capture; never the payload).
        if self.shutdown.load(Ordering::Acquire) {
            return Err(GitCoreError::Wire(format!(
                "sandboxed `git {}` cancelled by process shutdown",
                inv.argv.join(" ")
            )));
        }
        if result.timed_out {
            return Err(GitCoreError::Wire(format!(
                "sandboxed `git {}` timed out (wall-clock {}s ceiling) — refused",
                inv.argv.join(" "),
                self.limits.timeout_secs
            )));
        }
        match result.exit_code {
            Some(0) => Ok(WireOutput {
                stdout: result.stdout,
                status: 0,
            }),
            other => Err(GitCoreError::Wire(format!(
                "sandboxed `git {}` exited {:?}: {}",
                inv.argv.join(" "),
                other,
                String::from_utf8_lossy(&result.stderr)
            ))),
        }
    }
}

/// **The production `GitCore` for the wire-serving tier (CT-006b).** Composes the sandboxed
/// [`GitWireExecutor`] (wire/maintenance ops → canonical `git`, no-host-exec) with the in-process read
/// backend [`GixCore`] over the SAME on-disk root (read/diff/blame in libgit2). The serve/advertise
/// calls flow here; `RoutedGitCore` routes each op by the per-op capability table (wire → Shell,
/// read → Gix, 0 routing errors). The HTTP smart-transport listener that drives `advertise_refs` /
/// `serve` over the wire is CT-006c — this is the GitCore it stands on.
pub fn production_git_core(
    root: impl Into<PathBuf>,
    limits: ResourceLimits,
    hooks: RunnerHooks,
) -> RoutedGitCore<GitWireExecutor, GixCore<RootedResolver>> {
    let root = root.into();
    let exec = GitWireExecutor::new(
        root.clone(),
        limits,
        hooks,
        unavailable_git_wire_credential_issuer(),
    );
    let read = GixCore::new(RootedResolver::new(root));
    RoutedGitCore::new(exec, read)
}

/// Production Git core with an explicit live Identity credential issuer.
pub fn production_git_core_with_issuer(
    root: impl Into<PathBuf>,
    limits: ResourceLimits,
    hooks: RunnerHooks,
    credential_issuer: Arc<dyn GitWireCredentialIssuer>,
) -> RoutedGitCore<GitWireExecutor, GixCore<RootedResolver>> {
    let root = root.into();
    let exec = GitWireExecutor::new(root.clone(), limits, hooks, credential_issuer);
    let read = GixCore::new(RootedResolver::new(root));
    RoutedGitCore::new(exec, read)
}

/// Production core with a process-level cancellation flag shared by every per-request executor.
pub fn production_git_core_with_shutdown(
    root: impl Into<PathBuf>,
    limits: ResourceLimits,
    hooks: RunnerHooks,
    shutdown: Arc<AtomicBool>,
) -> RoutedGitCore<GitWireExecutor, GixCore<RootedResolver>> {
    let root = root.into();
    let exec = GitWireExecutor::new(
        root.clone(),
        limits,
        hooks,
        unavailable_git_wire_credential_issuer(),
    )
    .with_shutdown_signal(shutdown);
    let read = GixCore::new(RootedResolver::new(root));
    RoutedGitCore::new(exec, read)
}

/// Shutdown-aware production Git core with an explicit live Identity credential issuer.
pub fn production_git_core_with_shutdown_and_issuer(
    root: impl Into<PathBuf>,
    limits: ResourceLimits,
    hooks: RunnerHooks,
    shutdown: Arc<AtomicBool>,
    credential_issuer: Arc<dyn GitWireCredentialIssuer>,
) -> RoutedGitCore<GitWireExecutor, GixCore<RootedResolver>> {
    let root = root.into();
    let exec = GitWireExecutor::new(root.clone(), limits, hooks, credential_issuer)
        .with_shutdown_signal(shutdown);
    let read = GixCore::new(RootedResolver::new(root));
    RoutedGitCore::new(exec, read)
}

/// Serving-tier default composition (default limits + pass-through guarantee hooks). Credential
/// minting is deliberately unavailable, so wire operations fail closed until the composition root
/// uses [`production_git_core_with_issuer`]. The on-disk `root` is the SAME root the durable git
/// backend ([`crate::DurableGitBackend`]) writes/reads through.
pub fn production_git_core_default(
    root: impl Into<PathBuf>,
) -> RoutedGitCore<GitWireExecutor, GixCore<RootedResolver>> {
    production_git_core(
        root,
        GitWireExecutor::default_limits(),
        GitWireExecutor::serving_hooks(),
    )
}

/// Serving defaults plus cooperative process-shutdown cancellation.
pub fn production_git_core_default_with_shutdown(
    root: impl Into<PathBuf>,
    shutdown: Arc<AtomicBool>,
) -> RoutedGitCore<GitWireExecutor, GixCore<RootedResolver>> {
    production_git_core_with_shutdown(
        root,
        GitWireExecutor::default_limits(),
        GitWireExecutor::serving_hooks(),
        shutdown,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_git::core::RepoLoc;

    #[test]
    fn pre_cancelled_executor_never_reaches_runsc() {
        let shutdown = Arc::new(AtomicBool::new(true));
        let executor =
            GitWireExecutor::serving_default("/absent/git-root").with_shutdown_signal(shutdown);
        let error = executor
            .run(&WireInvocation {
                repo: RepoLoc::new("acme", "eu-west", "widgets"),
                argv: vec!["upload-pack".into(), "--stateless-rpc".into()],
                stdin: Vec::new(),
            })
            .expect_err("shutdown must refuse before filesystem or runsc access");
        assert!(error.to_string().contains("cancelled by process shutdown"));
    }

    #[test]
    fn production_default_refuses_without_a_live_credential_issuer() {
        let executor = GitWireExecutor::serving_default("/absent/git-root");
        let error = executor
            .run(&WireInvocation {
                repo: RepoLoc::new("acme", "eu-west", "widgets"),
                argv: vec!["upload-pack".into(), "--stateless-rpc".into()],
                stdin: Vec::new(),
            })
            .expect_err(
                "an unbound production executor must fail before filesystem or runsc access",
            );
        assert!(error
            .to_string()
            .contains("no live Identity Git-wire credential issuer is configured"));
    }
}

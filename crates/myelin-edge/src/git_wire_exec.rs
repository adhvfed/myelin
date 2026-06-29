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
    GitWireSpec, IdemToken, MeterTarget, ResourceLimits, RunTokenRef, RunnerHooks, SandboxBackend,
    SandboxLaunch,
};
use myelin_git::core::{GitCoreError, RoutedGitCore, WireExecutor, WireInvocation, WireOutput};
use myelin_git::gix_backend::{GixCore, RootedResolver};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

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
    /// Monotone per-invocation sequence → a unique idempotency token per sandboxed launch (the runner
    /// dedups on it; one sandbox per wire op, never reused).
    seq: AtomicU64,
}

impl GitWireExecutor {
    /// Build the production executor over an on-disk git root, resource limits, and the four-guarantee
    /// hooks. The caller (the composition root / CT-006c server) supplies real hooks; nothing here
    /// fabricates a guarantee.
    pub fn new(root: impl Into<PathBuf>, limits: ResourceLimits, hooks: RunnerHooks) -> Self {
        Self {
            backend: GvisorBackend::new(),
            root: root.into(),
            limits,
            hooks,
            seq: AtomicU64::new(0),
        }
    }

    /// Serving-tier default: sane resource bounds + the four-guarantee hooks wired to pass-through
    /// seams (the real reserve/settle wallet body is Commercial 11.7, attribution is Identity 4.7, and
    /// the isolation-floor body is the hardening profile the launch ALREADY asserts in force — see
    /// `launch_git_wire`). The composition root overrides the hooks to bind the live wallet/token/KMS.
    pub fn serving_default(root: impl Into<PathBuf>) -> Self {
        Self::new(root, Self::default_limits(), Self::serving_hooks())
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

    /// Synthesize the per-invocation four-guarantee tokens (one sandbox per wire op). The serving tier
    /// derives these from the verified request; here they are unique-per-launch handles.
    fn next_tokens(&self) -> (RunTokenRef, MeterTarget, IdemToken) {
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let tag = format!("git-wire-{}-{n}", std::process::id());
        (
            RunTokenRef {
                jti: format!("{tag}-jti"),
            },
            MeterTarget {
                reserve_id: format!("{tag}-reserve"),
            },
            IdemToken(tag),
        )
    }
}

impl WireExecutor for GitWireExecutor {
    fn run(&self, inv: &WireInvocation) -> Result<WireOutput, GitCoreError> {
        let (rt, mt, it) = self.next_tokens();
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
            .launch_git_wire(&spec, &self.hooks)
            .map_err(|e| GitCoreError::Wire(e.to_string()))?;

        // One sandbox per wire op — tear it down (idempotent; the guest has already exited).
        let _ = self.backend.kill(&handle);

        // Honor the exit/timeout: a timeout or a non-zero `git` exit is a HARD error, never a silent
        // empty stdout. stderr is folded into the error message (capped capture; never the payload).
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
    let exec = GitWireExecutor::new(root.clone(), limits, hooks);
    let read = GixCore::new(RootedResolver::new(root));
    RoutedGitCore::new(exec, read)
}

/// Serving-tier default composition (default limits + pass-through guarantee hooks; the live
/// wallet/token/KMS bodies are bound by the composition root). The on-disk `root` is the SAME root the
/// durable git backend ([`crate::DurableGitBackend`]) writes/reads through.
pub fn production_git_core_default(
    root: impl Into<PathBuf>,
) -> RoutedGitCore<GitWireExecutor, GixCore<RootedResolver>> {
    production_git_core(
        root,
        GitWireExecutor::default_limits(),
        GitWireExecutor::serving_hooks(),
    )
}

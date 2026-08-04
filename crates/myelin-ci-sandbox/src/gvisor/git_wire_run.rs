//! Running the sandboxed git smart-transport wire: the [`GitWireSpec`] launch surface, its OCI
//! config + bundle staging, and the bounded stdin/stdout container execution.

use super::*;
use crate::hardening::HardeningProfile;
use crate::redaction::RedactionPlan;
use crate::{CompletionSettlementOwner, EgressPolicy, IdemToken, ImageRef, JobKind, JobSpec, LaunchPermit, MeterTarget, ReserveHandle, ResourceLimits, ResourceUsage, RunTokenCredential, RunnerHooks, SandboxBackend, SandboxHandle, SandboxLaunch, TrustTier, WorkspaceSpec};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// CT-006a (GT-006 / SI-013) — the SANDBOXED GIT-WIRE capability.
//
// The git smart-transport wire (`upload-pack` = clone/fetch, `receive-pack` = push) is canonical
// `git` processing UNTRUSTED client pack/negotiation bytes, so it MUST run under the SAME proven
// hardening this backend already enforces for CI/agent jobs (ro-root, all-caps-dropped, no-new-privs,
// seccomp, no-netns egress-deny, non-root uid, bounded mem/pids/disk, whole-container kill + cleanup,
// bounded capture). On top of that floor the git wire needs THREE things this section adds, all by
// REUSING the machinery above:
//   1. the bare repo BIND-MOUNTED READ-ONLY at `/repo` (a serve can never mutate it) — a
//      [`GitWireMounts`] entry rendered into the SAME OCI `mounts` array as the `/tmp` tmpfs;
//   2. a WRITABLE QUARANTINE bind-mount at `/quarantine` for `receive-pack` object intake — the
//      sandboxed receive-pack writes objects THERE, the host-side ref-CAS policy (CT-006b) inspects it
//      AFTER the run; it NEVER touches the real repo;
//   3. BOUNDED stdin delivery (the stateless-rpc request body, capped at [`WIRE_STDIN_BOUND`]) piped to
//      the runsc child + captured stdout (the response) via the existing [`drain_capped`] bound.
//
// SECURITY — path confinement (the GT-001 isolation boundary, replicated): the (tenant, region, repo)
// locator is URL/client-influenced, so a raw `PathBuf::join` is a cross-tenant path-traversal breakout.
// [`resolve_bare_repo_path`] VALIDATES every segment against the allowlist `[A-Za-z0-9._-]` (no empty /
// `.` / `..` / separator / NUL / absolute) and REFUSES before any path is built — the byte-for-byte
// guarantee of `myelin_git::gix_backend::validate_path_segment` (the GT-001 fix). It is REPLICATED here
// (not imported) because `myelin-git` is a dev-dep only — the CI sandbox must carry NO production edge
// to the git crate (X-1 acyclic: CI emits, Git reads); a drift test in CT-006b pins the two in sync.
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// The fixed guest mount point for the READ-ONLY bare repo (the git argv ends with this path).
pub const WIRE_REPO_MOUNT: &str = "/repo";
/// The fixed guest mount point for the WRITABLE push-object quarantine (`receive-pack` intake).
pub const WIRE_QUARANTINE_MOUNT: &str = "/quarantine";

/// **The receive-pack PACK-INGEST script (CT-006d) — the in-sandbox writable-quarantine solution.**
///
/// The push write path's hardest constraint: a host bind-mounted `/quarantine` is NOT writable by the
/// in-guest non-root uid 65534 under rootless `runsc` (a write fails EINVAL on the gofer). So the
/// quarantine lives entirely in the guest's OWN writable `/tmp` tmpfs (writable by 65534 — the SAME
/// tmpfs `git upload-pack` already uses for `HOME`), and the ingested objects are streamed OUT to the
/// host on stdout (captured by the existing wire stdout streaming), NEVER through a host bind.
///
/// Step by step (busybox `sh`; the canonical `git` does ALL pack parsing — the UNTRUSTED client pack is
/// parsed only HERE, inside the hardened sandbox, never on the host):
///   1. `git init --bare /tmp/q` — a throwaway quarantine repo on the writable tmpfs.
///   2. `objects/info/alternates → /repo/objects` — so a THIN pack's delta bases (objects already in the
///      RO real repo) resolve. `/repo` stays READ-ONLY (alternates only READ it).
///   3. `git index-pack --stdin --fix-thin` — ingest the pushed pack from stdin into the quarantine's
///      `objects/pack/`, VALIDATING every object's sha + the pack checksum and resolving deltas against
///      the alternates. A corrupt/forged/incomplete pack (a base neither sent nor present) makes
///      `index-pack` exit non-zero → the launch reports the failure and the host rejects the push (no
///      objects migrate, no ref moves). Its progress/stats go to stderr (`1>&2`) so they never corrupt
///      the object stream on stdout.
///   4. `verify-pack -v … | awk` — list the oids the push INTRODUCED (exactly the objects in the received
///      pack); `git cat-file --batch` then streams each as a FULLY-RESOLVED raw object
///      (`<oid> SP <type> SP <size>\n<payload>\n`) on stdout — deltas already applied. The host parses
///      this trivial framing (NO pack indexer runs on the host over untrusted bytes), re-hashes each
///      object (a forged oid is impossible — git2's `odb.write` recomputes the sha), runs the in-process
///      policy + connectivity check, and ONLY THEN migrates the objects into the real repo under the
///      one-tx ref-CAS + outbox. `git receive-pack` is deliberately NEVER run server-side (it would
///      update the RO real repo's refs) — the in-sandbox git ONLY ingests bytes; the ref move is the
///      trusted in-process host code's job alone.
///
/// NO untrusted data is interpolated into this script — the pushed pack arrives only on stdin, never as
/// an argv/shell token; the ref-update commands are parsed by the host BEFORE the sandbox is invoked.
pub const RECEIVE_PACK_INGEST_SCRIPT: &str = "set -e
export HOME=/tmp
export GIT_CONFIG_NOSYSTEM=1
export GIT_EXEC_PATH=/usr/lib/git-core
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=safe.directory
export GIT_CONFIG_VALUE_0=*
export GIT_DIR=/tmp/q
rm -rf /tmp/q
git init --bare -q /tmp/q
printf '%s\\n' /repo/objects > /tmp/q/objects/info/alternates
git index-pack --stdin --fix-thin 1>&2
git verify-pack -v /tmp/q/objects/pack/pack-*.idx | awk '$2 ~ /^(commit|tree|blob|tag)$/ {print $1}' > /tmp/oids
git cat-file --batch < /tmp/oids
";
/// The documented cap on the stateless-rpc REQUEST BODY (stdin) the wire delivers into the guest —
/// 64 MiB. A larger body is REFUSED fail-closed before any container spawns (a client cannot force the
/// host to buffer an unbounded request). CT-006b may revisit this for very large pushes (chunked
/// intake); for CT-006a it bounds the negotiation/advertise request bodies with margin to spare.
pub const WIRE_STDIN_BOUND: usize = 64 * 1024 * 1024;
/// The DEFAULT generous cap on the git-wire RESPONSE (the upload-pack packfile / advertisement) the
/// container streams to stdout — 512 MiB, matching the serving tier's default `disk_bytes` scratch
/// quota. UNLIKE the 256 KiB [`SANDBOX_CAPTURE_BOUND`] used for CI/agent logs, the wire response is a
/// real packfile (megabytes for a real repo), so it gets this large bound — STREAMED to a host temp
/// file ([`drain_to_temp_file`]) so host RAM stays bounded to one chunk during the run. The LIVE cap is
/// derived per-launch from `spec.limits.disk_bytes` (so it is configurable via [`ResourceLimits`]); this
/// const documents the default. A response that exceeds the live cap is REFUSED loudly at the wire seam
/// ([`WireError::OutputTooLarge`]) — never a silently-truncated pack (which would fail the client's
/// `index-pack` with "early EOF"). True end-to-end streaming (no materialization) is a future
/// `WireOutput` streaming-body API change.
pub const WIRE_STDOUT_BOUND: usize = 512 * 1024 * 1024;

/// **A sandboxed git-wire invocation** — the git-shaped analogue of a [`JobSpec`]. It carries a
/// RESOLVER-VALIDATED bare-repo host path (RO-mounted at [`WIRE_REPO_MOUNT`]), the canonical `git` argv
/// (WITHOUT the repo path — the launch appends `/repo`), the bounded stdin request body, optional extra
/// env, an optional WRITABLE quarantine host dir (bound at [`WIRE_QUARANTINE_MOUNT`]), and the limits +
/// four-guarantee tokens. Build it with [`GitWireSpec::for_repo`], which performs the path confinement.
#[derive(Clone, Debug)]
pub struct GitWireSpec {
    repo_host_path: PathBuf,
    /// The on-disk root the locator resolved UNDER — retained so the launch can assert (defence in
    /// depth, CT-006b) that the resolved repo path stays a REAL directory beneath the canonicalized
    /// root before it is bind-mounted (a symlinked `<repo>.git` / component cannot escape the tree).
    root: PathBuf,
    pub(super) git_argv: Vec<String>,
    pub(super) stdin: Vec<u8>,
    env: Vec<String>,
    quarantine_host_path: Option<PathBuf>,
    limits: ResourceLimits,
    run_token: RunTokenCredential,
    meter_to: MeterTarget,
    idem_token: IdemToken,
}

impl GitWireSpec {
    /// Build a git-wire spec for `(root, tenant, region, repo)`, RESOLVING + VALIDATING the bare-repo
    /// path through [`resolve_bare_repo_path`] (fail-closed on any cross-tenant/`..`/absolute segment —
    /// the locator NEVER reaches a mount raw). `git_argv` is the canonical subcommand + flags WITHOUT
    /// the repo path (e.g. `["upload-pack", "--stateless-rpc", "--advertise-refs"]`); the launch appends
    /// `/repo`. `quarantine_host_path` (if set) is bound WRITABLE at `/quarantine` for receive-pack.
    #[allow(clippy::too_many_arguments)]
    pub fn for_repo(
        root: &Path,
        tenant: &str,
        region: &str,
        repo: &str,
        git_argv: Vec<String>,
        stdin: Vec<u8>,
        env: Vec<String>,
        quarantine_host_path: Option<PathBuf>,
        limits: ResourceLimits,
        run_token: RunTokenCredential,
        meter_to: MeterTarget,
        idem_token: IdemToken,
    ) -> Result<GitWireSpec, WireError> {
        let repo_host_path = resolve_bare_repo_path(root, tenant, region, repo)?;
        Ok(GitWireSpec {
            repo_host_path,
            root: root.to_path_buf(),
            git_argv,
            stdin,
            env,
            quarantine_host_path,
            limits,
            run_token,
            meter_to,
            idem_token,
        })
    }

    /// The resolver-validated bare-repo host path that will be RO-mounted at `/repo`.
    pub fn repo_host_path(&self) -> &Path {
        &self.repo_host_path
    }
}

impl GvisorBackend {
    /// **Run a canonical-`git` wire op in the hardened gVisor sandbox (CT-006a).** Drives the SAME
    /// four-guarantee seam as [`launch`](SandboxBackend::launch) (isolation floor → hardening assert →
    /// reserve → final attribution → run → settle), with the git-wire additions: the bare repo is bound
    /// READ-ONLY at `/repo`, an optional writable quarantine at `/quarantine`, the bounded request body
    /// is piped to stdin, and the response is captured (bounded). The command is `git <argv> /repo`,
    /// run under the full hardening (ro-root, caps dropped, no-new-privs, seccomp, no-netns, non-root
    /// uid, mem/pids/disk bounded, whole-container kill + cleanup). Fail-closed: an oversize body, an
    /// unmet floor / exhausted wallet, or an absent git rootfs all REFUSE before/instead of a result.
    pub fn launch_git_wire(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
    ) -> Result<SandboxLaunch, WireError> {
        // The guest command: `git <argv> /repo` (the RO repo mount is the final argument).
        let mut command = Vec::with_capacity(spec.git_argv.len() + 2);
        command.push("git".to_string());
        command.extend(spec.git_argv.iter().cloned());
        command.push(WIRE_REPO_MOUNT.to_string());
        self.launch_git_command(spec, hooks, command, &NEVER_CANCELLED)
    }

    /// Git-wire launch with cooperative process-shutdown cancellation. Once `cancellation` becomes
    /// true, the wait loop kills container PID 1 and reaps `runsc` exactly like a wall timeout, but
    /// does not misreport the operator-requested cancellation as a timeout.
    pub fn launch_git_wire_until_cancelled(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
        cancellation: &AtomicBool,
    ) -> Result<SandboxLaunch, WireError> {
        let mut command = Vec::with_capacity(spec.git_argv.len() + 2);
        command.push("git".to_string());
        command.extend(spec.git_argv.iter().cloned());
        command.push(WIRE_REPO_MOUNT.to_string());
        self.launch_git_command(spec, hooks, command, cancellation)
    }

    /// **Ingest a pushed packfile in the hardened sandbox (CT-006d — the push write path's untrusted-pack
    /// intake).** The pushed pack is piped to stdin; the guest runs [`RECEIVE_PACK_INGEST_SCRIPT`]
    /// (`git index-pack --fix-thin` into a writable `/tmp` tmpfs quarantine, NEVER the RO `/repo`) and
    /// streams the VALIDATED, self-contained quarantine pack back on stdout. The host then stages that
    /// pack in a throwaway odb, runs the in-process policy + fsck, and migrates it under the one-tx
    /// ref-CAS — the in-sandbox `git` ONLY ingests bytes, it never moves a ref or touches the real repo.
    /// Same four-guarantee seam + hardening floor + RO `/repo` mount as [`Self::launch_git_wire`].
    pub fn launch_git_receive_pack(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
    ) -> Result<SandboxLaunch, WireError> {
        // The guest entrypoint is busybox `sh` running the FIXED ingest script — no `/repo` is appended
        // (the script references the RO `/repo` mount itself, for the thin-pack alternates), and no
        // untrusted data is interpolated (the pack arrives only on stdin).
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            RECEIVE_PACK_INGEST_SCRIPT.to_string(),
        ];
        self.launch_git_command(spec, hooks, command, &NEVER_CANCELLED)
    }

    /// Receive-pack ingest with the same cooperative shutdown cancellation as wire serving.
    pub fn launch_git_receive_pack_until_cancelled(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
        cancellation: &AtomicBool,
    ) -> Result<SandboxLaunch, WireError> {
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            RECEIVE_PACK_INGEST_SCRIPT.to_string(),
        ];
        self.launch_git_command(spec, hooks, command, cancellation)
    }

    /// The shared git-wire launch body (CT-006a/d): bound stdin, symlink-confine the repo, build the
    /// hardened internal `JobSpec`, drive the four-guarantee seam, mount the RO repo (+ optional writable
    /// quarantine bind), run the container streaming the bounded request body to stdin + capturing the
    /// bounded response from stdout. `command` is the fully-built guest argv (`git <argv> /repo` for the
    /// wire serve, or the `sh -c <ingest>` for receive-pack) — the ONLY thing that differs between callers.
    fn launch_git_command(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
        command: Vec<String>,
        cancellation: &AtomicBool,
    ) -> Result<SandboxLaunch, WireError> {
        let job = build_git_wire_job(spec, command, cancellation)?;

        // The git wire is a direct synchronous API call (an HTTP-shaped request/response), NOT a
        // `RunnerAgent`-mediated job with a terminal reporter parked above it. There is no
        // `report_retryable_attempt` mechanism this path can route a post-commit failure through —
        // refuse loudly here, before reserve, rather than silently mis-defer a real measured attempt
        // the way `TerminalReporter` ownership would (Sol's finding: git-wire must require `Hook`
        // ownership, unconditionally).
        if hooks.completion_settlement_owner() == CompletionSettlementOwner::TerminalReporter {
            return Err(WireError::Runtime(
                "git-wire launch requires Hook-owned completion settlement — it is a direct \
                 synchronous path with no terminal reporter above it to defer a retryable-attempt \
                 accounting to"
                    .to_string(),
            ));
        }

        // Derive the isolation/config posture before the final mutable launch boundary.
        hooks.enforce_isolation_floor(&job)?;
        let (cfg, rootfs) = build_git_wire_oci_config(&job, spec)?;
        let reserve = hooks.reserve(&job)?;
        // Thread the REAL launch permit through to `run_and_capture` (Sol's fix): the old
        // `hooks.attribute(&job)` eagerly committed-and-released attribution HERE, before any
        // spawn attempt — decoupling the durable commit from the actual OS spawn entirely, which
        // made every subsequent `RunFailure` phase from `run_git_wire_container` structurally
        // mislabeled (e.g. a post-exec pipe failure would be reported `Uncommitted` even though
        // attribution had already, durably, committed). Acquiring (not immediately committing) the
        // permit and passing it into `run_and_capture` makes the SAME durable-launch-commit gate
        // that the CI/agent path uses also govern the git-wire spawn, so its phase reporting
        // becomes truthful.
        let launch_permit = match hooks.acquire_launch_permit(&job) {
            Ok(permit) => permit,
            Err(attribute_error) => {
                hooks.release_unused(&job, &reserve)?;
                return Err(attribute_error.into());
            }
        };

        // Same pre-existing leak, same fix, as `launch_with` above: `run_git_wire_container`'s
        // failure carries the phase the launch reached, so the reservation is released/settled
        // correctly instead of leaking on every failure path. Git-wire has no terminal reporter
        // above it (refused up front, above) so every post-commit phase settles synchronously here
        // — there is no `RetryableAttempt`/reporter path to defer to.
        let (
            ContainerRun {
                child,
                bundle_dir,
                result,
                run_error,
            },
            stdout_truncated,
        ) = match run_git_wire_container(
            &job,
            &cfg,
            spec.stdin.clone(),
            &rootfs,
            cancellation,
            launch_permit,
        ) {
            Ok(run_and_truncated) => run_and_truncated,
            Err(run_failure) => {
                return Err(dispose_git_wire_run_failure(
                    hooks,
                    &job,
                    &reserve,
                    run_failure,
                ))
            }
        };

        // FAIL LOUD at the seam (CT-006c FU-1): if the response overflowed the generous wire cap, the
        // captured pack is TRUNCATED — refuse rather than hand back a short pack the client's
        // `index-pack` would reject with "early EOF". A real execution genuinely happened (this is
        // deterministic for the same request/limit, never retryable), so settle its already-measured
        // usage SYNCHRONOUSLY before returning — never leave a real, completed attempt unsettled.
        // Tear down the just-run container's bundle (the container itself is already deleted by
        // `run_git_wire_container`).
        if stdout_truncated {
            let _ = std::fs::remove_dir_all(&bundle_dir);
            if let Err(settle_error) = hooks.settle_completed(&job, &reserve, result.usage) {
                return Err(WireError::Runtime(format!(
                    "response exceeded the wire cap AND settling its measured usage also failed \
                     ({settle_error}) — reservation may be leaked"
                )));
            }
            return Err(WireError::OutputTooLarge {
                cap: job.limits.disk_bytes as usize,
            });
        }

        let guest_id = format!("runsc-gitwire-{}", job.idem_token.0);
        self.live
            .lock()
            .unwrap()
            .insert(guest_id.clone(), RunscProc { child, bundle_dir });

        // Settle against the REAL measured usage (never interrupt in-flight).
        if let Err(error) = hooks.settle_completed(&job, &reserve, result.usage) {
            let _ = self.kill(&SandboxHandle {
                guest_id: guest_id.clone(),
            });
            return Err(error.into());
        }

        if let Some(error) = run_error {
            let _ = self.kill(&SandboxHandle { guest_id });
            return Err(WireError::Runtime(error));
        }

        Ok(SandboxLaunch {
            handle: SandboxHandle { guest_id },
            result,
            output_complete: true,
        })
    }
}

/// Shared, hooks-free git-wire request validation + `JobSpec` construction (CT-007 slice 5b.3-3):
/// the piece [`GvisorBackend::launch_git_command`] (the standalone, billed git-wire path) and the
/// parent-attempt Hop A transport ([`fetch_checkout_pack_within_parent_attempt`]) both need
/// identically, with NO reserve/permit/settle of any kind. An `Err` here means nothing has spawned —
/// both callers can treat it as genuinely free.
pub(super) fn build_git_wire_job(
    spec: &GitWireSpec,
    command: Vec<String>,
    cancellation: &AtomicBool,
) -> Result<JobSpec, WireError> {
    if cancellation.load(Ordering::Acquire) {
        return Err(WireError::Runtime(
            "Git wire launch cancelled by process shutdown".into(),
        ));
    }
    // Bound the request body BEFORE anything spawns (a client cannot force unbounded host buffering).
    if spec.stdin.len() > WIRE_STDIN_BOUND {
        return Err(WireError::StdinTooLarge {
            len: spec.stdin.len(),
            cap: WIRE_STDIN_BOUND,
        });
    }
    // Symlink-path defence in depth (CT-006b 4a): the resolved repo MUST be a real directory under
    // the canonicalized root before it is bind-mounted (a symlinked `<repo>.git`/component cannot
    // make the RO mount follow out of the tenant tree). REFUSED here, before any container spawns.
    assert_repo_under_root(&spec.root, &spec.repo_host_path)?;
    // Build the internal hardened JobSpec so the git wire inherits the SAME profile + guarantees as
    // every CI/agent job (the image is a placeholder — gVisor runs the staged rootfs, not an image).
    let image = ImageRef::pinned(
        "sandbox/git-wire@sha256:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .map_err(|e| WireError::Runtime(e.to_string()))?;
    JobSpec::new(
        JobKind::Ci,
        image,
        command,
        vec![],
        vec![],
        EgressPolicy::deny_all(), // a serve needs no egress (egress-deny / no-netns).
        spec.limits,
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        spec.run_token.clone(),
        spec.meter_to.clone(),
        spec.idem_token.clone(),
    )
    .map_err(|e| WireError::Runtime(e.to_string()))
}

/// Shared, hooks-free hardening-profile derivation + OCI config construction (CT-007 slice 5b.3-3) —
/// the other half of [`build_git_wire_job`]'s split, separated only so the standalone path can still
/// interleave its own `hooks`-dependent `enforce_isolation_floor` check between the two (byte-identical
/// check ORDER to before this extraction). Returns the config plus the resolved (possibly-absent;
/// `run_git_wire_container` fails closed on that) rootfs path.
pub(super) fn build_git_wire_oci_config(
    job: &JobSpec,
    spec: &GitWireSpec,
) -> Result<(OciConfig, PathBuf), WireError> {
    let profile = HardeningProfile::derive(job);
    profile.assert_enforced().map_err(WireError::Hardening)?;

    // The staged rootfs is referenced by an ABSOLUTE `root.path` (no symlink — required alongside
    // the host bind mounts). Verify its complete mountpoint set and canonical-tree pin before the
    // config can reach any spawn path; no env-path-only fallback remains.
    let rootfs = verified_gvisor_git_rootfs().map_err(WireError::Runtime)?;
    let cfg = OciConfig::from_spec(job, &profile)
        .with_extra_env(spec.env.clone())
        .with_rootless_host_mounts(
            rootfs.clone(),
            spec.repo_host_path.clone(),
            spec.quarantine_host_path.clone(),
        )
        .map_err(WireError::Runtime)?;
    Ok((cfg, rootfs))
}

/// Dispose of a post-reserve [`RunFailure`] from `run_git_wire_container` into the correct
/// [`WireError`], settling/releasing the reservation along the way wherever that is safe.
/// `CommitOutcomeUnknown` still settles/releases NOTHING here, same as gVisor's own
/// [`GvisorBackend::dispose_run_failure`] — the durable commit outcome is genuinely unknown
/// regardless of backend. What differs is the OTHER three phases: git-wire has NO terminal
/// reporter above it (`launch_git_command` refuses reporter-owned hooks before reserve), so
/// `CommittedButNotExecuted`/`Executed` always settle synchronously here — there is no
/// `RetryableAttempt`/reporter path to ever defer to. Extracted as a standalone function (rather
/// than only inlined in `launch_git_command`) so it is unit-testable without a real `runsc` binary.
fn dispose_git_wire_run_failure(
    hooks: &RunnerHooks,
    job: &JobSpec,
    reserve: &ReserveHandle,
    run_failure: RunFailure,
) -> WireError {
    let message = run_failure.to_string();
    match run_failure {
        RunFailure::Uncommitted { .. } => {
            if let Err(settle_error) = hooks.release_unused(job, reserve) {
                return WireError::Runtime(format!(
                    "run_git_wire_container() failed (uncommitted: {message}) AND \
                     release_unused also failed ({settle_error}) — reservation may be leaked"
                ));
            }
            WireError::Runtime(message)
        }
        RunFailure::CommitOutcomeUnknown { .. } => {
            // Neither release nor settle — the durable commit outcome is genuinely unknown;
            // guessing either way misaccounts a real reservation.
            WireError::Runtime(format!(
                "durable launch commit outcome unknown, needs reconciliation: {message}"
            ))
        }
        RunFailure::CommittedButNotExecuted { .. } => {
            let zero = ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            };
            if let Err(settle_error) = hooks.settle_completed(job, reserve, zero) {
                return WireError::Runtime(format!(
                    "run_git_wire_container() failed (committed but not executed: {message}) \
                     AND its zero-usage settlement also failed ({settle_error}) — reservation \
                     may be leaked"
                ));
            }
            WireError::Runtime(message)
        }
        RunFailure::Executed { usage, .. } => {
            if let Err(settle_error) = hooks.settle_completed(job, reserve, usage) {
                return WireError::Runtime(format!(
                    "run_git_wire_container() failed (executed: {message}) AND its \
                     conservative-usage settlement also failed ({settle_error}) — reservation \
                     may be leaked"
                ));
            }
            WireError::Runtime(message)
        }
    }
}

/// The git-wire production run path — mirrors [`run_production_container`] but stages the GIT-bearing
/// rootfs ([`resolved_gvisor_git_rootfs`]) and pipes the bounded request body to the container's stdin.
/// The bind mounts (RO repo + optional writable quarantine) are already in `cfg`'s `mounts` array. The
/// container + bundle are cleaned up on EVERY path (no leaks); an absent git rootfs fails closed.
fn run_git_wire_container(
    job: &JobSpec,
    cfg: &OciConfig,
    stdin: Vec<u8>,
    rootfs: &Path,
    cancellation: &AtomicBool,
    launch_permit: LaunchPermit,
) -> Result<(ContainerRun, bool), RunFailure> {
    // The standalone billed path drops the second tuple element (Sol's round-3 review: "the
    // standalone wrapper may collapse it back for compatibility") — its own `hooks.release_unused`
    // contract already only ever attempted best-effort bundle cleanup, never independently verified
    // it, so this is byte-identical to its prior behavior.
    let (finalization, _bundle_cleanup_proof) =
        run_git_wire_container_raw(job, cfg, stdin, rootfs, cancellation, launch_permit);
    settle_finalization(
        finalization?,
        |(run, _): &(ContainerRun, bool)| run.result.usage,
        |(run, _): (ContainerRun, bool), teardown| {
            discard_container_run_after_teardown_failure(run, teardown)
        },
    )
}

/// A git-wire hop's pre-settlement outcome: `Finalized` means teardown was independently proven
/// (whichever way the hop's own `Result` came out); `Failed` means teardown itself could not be
/// proven, alongside whatever the hop's own `Result` was. Named (rather than inlined) purely to keep
/// clippy's `type_complexity` lint quiet — the meaning is exactly [`RuntimeFinalization`]'s.
pub(super) type GitWireHopFinalization = RuntimeFinalization<Result<(ContainerRun, bool), RunFailure>>;

/// Whether [`run_git_wire_container_raw`]'s OWN bundle-dir cleanup — on every path that removes it
/// BEFORE the caller ever sees a live [`ContainerRun`] to retire itself — was independently proven
/// (CT-007 slice 5b.3-3, Sol's round-3 review, blocker 1). `Ok(())` covers BOTH "nothing was ever
/// created" and "removal verified"; `Err` means a bundle directory may still be sitting on disk with
/// nobody now responsible for it. Deliberately carried OUTSIDE `RunFailure`/`RuntimeTeardownError`
/// rather than folded into either: both are shared with EVERY other `finalize_and_merge`/
/// `settle_finalization` caller in this file, and widening either just to carry this one
/// git-wire-specific fact would ripple into unrelated callers for no benefit. The standalone billed
/// path ([`run_git_wire_container`]) simply drops this value (see its own doc); the parent-attempt
/// transport must not — an unproven value there forces a `TeardownUnproven` disposition regardless of
/// what the `RunFailure`/`RuntimeFinalization` half would otherwise say.
pub(super) type BundleCleanupProof = Result<(), String>;

/// The pre-settlement half of [`run_git_wire_container`] (CT-007 slice 5b.3-3, Sol's review, blocker
/// 2): identical body, but returns the [`RuntimeFinalization`] BEFORE `settle_finalization` collapses
/// it into a bare `Result<(ContainerRun, bool), RunFailure>` — a collapse that is lossy for a caller
/// that needs to distinguish "the run failed for its own reason" from "the run's own result was fine
/// but teardown itself could not be independently proven." The standalone path
/// ([`run_git_wire_container`], above) never needed that distinction (it always settles/releases
/// through `hooks` either way) — the parent-attempt Hop A transport does, since a teardown-unproven
/// outcome must surface as [`CheckoutTransportError::TeardownUnproven`], never silently folded into
/// an ordinary `Failed`. Outer `Result` mirrors [`run_production_container_streaming`]'s established
/// shape: a pre-finalize failure (absent rootfs, bad OCI layout, bundle staging, cgroup creation) is
/// unconditionally `Uncommitted` — `finalize_runtime` was never reached, so there is no teardown
/// question to represent at all. The paired [`BundleCleanupProof`] (Sol's round-3 review, blocker 1)
/// is `Ok(())` whenever nothing needed removing yet or removal was verified, `Err` whenever THIS
/// function's own best-effort bundle-dir cleanup could not be confirmed.
pub(super) fn run_git_wire_container_raw(
    job: &JobSpec,
    cfg: &OciConfig,
    stdin: Vec<u8>,
    rootfs: &Path,
    cancellation: &AtomicBool,
    launch_permit: LaunchPermit,
) -> (
    Result<GitWireHopFinalization, RunFailure>,
    BundleCleanupProof,
) {
    let bin = runsc_bin();
    if !rootfs.exists() {
        return (
            Err(RunFailure::uncommitted(format!(
                "staged gVisor git rootfs absent: {} (the git wire REQUIRES a real `git` in the \
                 guest — stage a git-bearing rootfs and point {ENV_GVISOR_GIT_ROOTFS} at it; see \
                 tests/git_wire_prod_exec_test.rs)",
                rootfs.display()
            ))),
            Ok(()), // nothing was ever staged
        );
    }
    // CT-007 slice 3, piece 7b (Sol's round-3 review): validated BEFORE staging anything, exactly
    // like the production path — `run_git_wire_container`'s own signature does not structurally
    // restrict `cfg` to `RootlessWithHostMounts` (only its current caller always builds one), so
    // this invariant must be enforced here, not merely true by convention. 7b never constructs a
    // non-Rootless prepared mode in production — 7c adds that constructor.
    let prepared_mode = PreparedRuntimeMode::Rootless;
    let mode = match require_oci_layout_matches_prepared_mode(cfg, &prepared_mode) {
        Ok(mode) => mode,
        Err(e) => return (Err(RunFailure::uncommitted(e)), Ok(())), // nothing was ever staged
    };

    // Stage a config-only bundle: `cfg`'s `root.path` is the ABSOLUTE staged rootfs (set by
    // `launch_git_wire`), so no `rootfs` symlink is staged (a symlinked root.path + a host bind mount
    // makes the rootless gofer fail to start the sandbox; an absolute root.path + bind mount works).
    let bundle_dir = match stage_git_wire_bundle(cfg) {
        Ok(dir) => dir,
        Err(e) => {
            let cleanup_proof = if e.leaked {
                Err(e.message.clone())
            } else {
                Ok(())
            };
            return (Err(RunFailure::uncommitted(e.message)), cleanup_proof);
        }
    };
    let container_id = format!("myelin-gitwire-{}-{}", std::process::id(), unique_suffix());

    // CT-007 slice 3, piece 7b: cgroup ownership hoisted up here (see
    // `run_production_container_streaming`'s identical treatment) — `run_and_capture` only
    // borrows it now, and this function owns the checked teardown via `finalize_runtime`.
    let cgroup = match MemoryCgroup::create(job.limits.mem_bytes, job.limits.cpu_millis) {
        Ok(cgroup) => cgroup,
        Err(e) => {
            let cleanup_proof = std::fs::remove_dir_all(&bundle_dir)
                .map_err(|re| format!("bundle dir {bundle_dir:?} removal failed: {re}"));
            return (Err(RunFailure::uncommitted(e)), cleanup_proof);
        }
    };

    let timeout = Duration::from_secs(job.limits.timeout_secs as u64);
    // The git-wire response (the packfile / advertisement) is STREAMED to a host temp file under a
    // GENEROUS cap derived from the job's `disk_bytes` scratch quota (configurable; default
    // [`WIRE_STDOUT_BOUND`] = 512 MiB) — NOT the 256 KiB CI/agent log bound — so a real-size pack comes
    // through whole while host RAM stays bounded to one chunk. Over the cap ⇒ `outcome.stdout_truncated`,
    // which the caller turns into a LOUD [`WireError::OutputTooLarge`] (never a silently-short pack).
    let wire_cap = job.limits.disk_bytes as usize;
    let (result, child_retirement) = run_and_capture(
        bin,
        &bundle_dir,
        &container_id,
        timeout,
        job.limits.mem_bytes,
        RunCaptureOptions {
            stdin: Some(StdinSource::Bytes(stdin)),
            stdout_mode: StdoutMode::StreamToFile { bound: wire_cap },
            cancellation,
            redaction: RedactionPlan::none(),
            output: None,
        },
        Some(launch_permit),
        mode,
        &cgroup,
    );
    let (primary, cleanup_proof): (Result<(ContainerRun, bool), RunFailure>, BundleCleanupProof) =
        match result {
            Ok(outcome) => {
                let stdout_truncated = outcome.stdout_truncated;
                // The git-wire path's stdout is the git smart-transport packfile/protocol stream
                // (StreamToFile), NOT job LOG output — it never reaches the durable log pipeline, and
                // masking arbitrary bytes in a binary packfile would corrupt it. So this path NEVER
                // redacts (an explicit `none()`, not `for_job`) — a deliberate distinction that matters
                // the day CI-1 injection makes `for_job` non-empty. Boundary redaction is a LOG-path
                // concern (the CappedHead capture in `run_production_container`), not a transport-path
                // concern.
                let result = build_result(job, &outcome, &RedactionPlan::none());
                (
                    Ok((
                        ContainerRun {
                            child: Box::new(SpawnedRunsc {
                                bin,
                                container_id: container_id.clone(),
                                mode,
                            }),
                            bundle_dir,
                            result,
                            run_error: outcome.stream_error,
                        },
                        stdout_truncated,
                    )),
                    // The bundle dir now lives on inside `ContainerRun` for the caller to retire —
                    // nothing was removed here, so there is nothing yet to (dis)prove.
                    Ok(()),
                )
            }
            Err(e) => {
                let cleanup_proof = std::fs::remove_dir_all(&bundle_dir)
                    .map_err(|re| format!("bundle dir {bundle_dir:?} removal failed: {re}"));
                (Err(e), cleanup_proof)
            }
        };
    (
        Ok(finalize_and_merge(
            primary,
            bin,
            &container_id,
            &prepared_mode,
            cgroup,
            RUNTIME_QUIESCE_TIMEOUT,
            child_retirement,
        )),
        cleanup_proof,
    )
}

/// A [`stage_config_only_bundle`] failure (CT-007 slice 5b.3-3, Sol's round-3 review): `message` is
/// the failure text; `leaked` is `true` iff a bundle directory may still exist on disk despite the
/// failure — this function's own best-effort cleanup of ITS OWN partially-staged directory could not
/// be verified. `false` means either nothing was ever created, or it was verified removed.
pub(super) struct StageBundleError {
    pub(super) message: String,
    leaked: bool,
}

impl std::fmt::Display for StageBundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Stage a CONFIG-ONLY OCI bundle (just `config.json`) — the rootfs is referenced by the config's
/// ABSOLUTE `root.path`, so no `rootfs` symlink is staged. `label` only names the temp-dir prefix
/// (for operator readability, e.g. `ps`/`ls /tmp` output) — it carries no security meaning. Returns
/// the bundle dir (removed on teardown). Sol's round-3 review: previously, a `config.json` write
/// failure left the just-created directory behind unconditionally — now best-effort cleaned up, with
/// the outcome reported via [`StageBundleError::leaked`] rather than silently discarded.
pub(super) fn stage_config_only_bundle(cfg: &OciConfig, label: &str) -> Result<PathBuf, StageBundleError> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    let bundle = std::env::temp_dir().join(format!(
        "myelin-{label}-bundle-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&bundle)
        .map_err(|e| StageBundleError {
            message: format!("create bundle dir {bundle:?}: {e}"),
            leaked: false,
        })?;
    let json = cfg.to_json_zeroizing().map_err(|message| {
        let (message, leaked) = match std::fs::remove_dir_all(&bundle) {
            Ok(()) => (message, false),
            Err(cleanup) => (
                format!(
                    "{message} AND cleaning up the partially-staged bundle dir also failed: \
                     {cleanup}"
                ),
                true,
            ),
        };
        StageBundleError { message, leaked }
    })?;
    let write_result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(bundle.join("config.json"))
        .and_then(|mut file| {
            file.write_all(json.as_bytes())
                .and_then(|()| file.sync_all())
        });
    if let Err(e) = write_result {
        return Err(match std::fs::remove_dir_all(&bundle) {
            Ok(()) => StageBundleError {
                message: format!("write config.json: {e}"),
                leaked: false,
            },
            Err(cleanup_err) => StageBundleError {
                message: format!(
                    "write config.json: {e} AND cleaning up the partially-staged bundle dir also \
                     failed: {cleanup_err}"
                ),
                leaked: true,
            },
        });
    }
    Ok(bundle)
}

/// Stage a CONFIG-ONLY OCI bundle for the git wire — see [`stage_config_only_bundle`].
fn stage_git_wire_bundle(cfg: &OciConfig) -> Result<PathBuf, StageBundleError> {
    stage_config_only_bundle(cfg, "gitwire")
}

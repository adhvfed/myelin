//! Running the production container: bundle staging, the prepared-mode/OCI-layout agreement check,
//! and the streaming run body every compute/checkout workload goes through.

use super::*;
use crate::user_namespace::{RunscInvocationMode, UserNamespaceConfig};
use crate::{JobSpec, LaunchPermit, SandboxCancellation, SandboxOutputSink};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// The REAL production run path (CT-002b → P-544): build a hardened OCI bundle from the spec and RUN
/// the untrusted `spec.command` inside a REAL `runsc` (gVisor) sandbox, capturing its REAL result.
///
/// **Why gVisor needs no forge defense (unlike Firecracker):** `runsc run` returns the container
/// process's REAL exit status directly to THIS host process — there is NO shared serial console the
/// workload could write — so the exit code is taken from the `runsc` child's real `ExitStatus`, never
/// from any in-container output. stdout/stderr are the runtime's OWN piped fds (two separate streams),
/// not a channel the host trusts the payload to frame; there is no nonce/base64 framing because there
/// is nothing for the payload to forge. The workload STILL runs NON-ROOT (`process.user` =
/// 65534/65534 in the OCI config) — defense in depth; untrusted code is never uid 0 in the sandbox.
///
/// Mechanism (REUSING the proven bundle pattern from `escape_drill_gvisor_test::stage_bundle`):
/// 1. Stage a temp bundle dir: a `rootfs` symlink → the caller-supplied, ALREADY-VERIFIED rootfs
///    path (`GvisorBackend::launch_with` resolved + digest-verified it from `spec.image` against the
///    [`GvisorAssetRegistry`](crate::asset_registry::GvisorAssetRegistry) BEFORE calling here — CT-007
///    gate 2/4; this function no longer calls [`resolved_gvisor_rootfs`] itself) + a `config.json` =
///    [`OciConfig::to_json`] (read-only root, caps dropped, no-new-privs, seccomp, no netns when
///    egress is default-deny, pids ceiling, NON-ROOT user, an advisory `memory.limit`, and a
///    size-bounded writable `/tmp` tmpfs from `spec.limits.disk_bytes` — CT-003a: a disk fill hits
///    ENOSPC at the quota, never an unbounded host-RAM-backed tmpfs).
///    1b. CT-003b (SI-017): establish an OUT-OF-BAND [`MemoryCgroup`] capped at `spec.limits.mem_bytes`
///    and place the `runsc` process tree (frontend + the sentry/gofer it forks) into it. This is the
///    REAL memory bound — rootless runsc ignores the OCI `memory.limit`, so without this an untrusted
///    job's anonymous memory was UNBOUNDED (a host-DoS escape). FAIL-CLOSED: if the cgroup cannot be
///    established the run REFUSES (never runs the workload unbounded).
/// 2. Run `runsc --rootless --network=none run -bundle <dir> <cid>` with stdout/stderr piped, waiting
///    at most `spec.limits.timeout_secs`; on expiry the WHOLE CONTAINER is killed (`runsc kill <cid>
///    KILL` + the child) ⇒ `timed_out=true`, `exit_code=None`.
/// 3. CT-007 slice 3, piece 7b: CHECKED `runsc delete -force <cid>` + verified [`MemoryCgroup`]
///    quiescence (via [`finalize_runtime`]), not merely best-effort — a teardown failure now
///    changes the disposition `launch_with` sees (see [`settle_finalization`]). The bundle dir is
///    removed on every path (no leaks).
pub(super) fn run_production_container(
    spec: &JobSpec,
    cfg: &OciConfig,
    launch_permit: LaunchPermit,
    rootfs: &Path,
    container_id: &str,
    prep: RuntimePreparation<'_>,
) -> Result<RuntimeFinalization<Result<ContainerRun, RunFailure>>, RunFailure> {
    run_production_container_streaming(
        spec,
        cfg,
        launch_permit,
        rootfs,
        container_id,
        None,
        SandboxCancellation::new(),
        prep,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_production_container_streaming(
    spec: &JobSpec,
    cfg: &OciConfig,
    launch_permit: LaunchPermit,
    rootfs: &Path,
    container_id: &str,
    output: Option<Arc<dyn SandboxOutputSink>>,
    cancellation: SandboxCancellation,
    mut prep: RuntimePreparation<'_>,
) -> Result<RuntimeFinalization<Result<ContainerRun, RunFailure>>, RunFailure> {
    let bin = runsc_bin();
    // `rootfs` is the ALREADY-VERIFIED path `GvisorBackend::launch_with` resolved from `spec.image`
    // via the `GvisorAssetRegistry` (CT-007 gate 2/4) — this production run path no longer calls
    // `resolved_gvisor_rootfs()` itself. That resolver remains the resolution mechanism the
    // registry's own construction (`GvisorAssetRegistry::from_bindings`, called once by
    // `production_gvisor_registry()`) uses, plus test scaffolding and the git-wire path's OWN
    // separate resolver — ordinary per-launch code no longer calls it directly. Honest fail-closed: a
    // runtime/start precondition failure surfaces as an error (never a fabricated exit). An absent
    // rootfs cannot produce a valid bundle (defense in depth — construction-time verification already
    // checked this, but a TOCTOU window between verification and staging is still handled honestly).
    // Both errors below occur before any spawn attempt — Uncommitted.
    if !rootfs.exists() {
        return Err(RunFailure::uncommitted(format!(
            "staged gVisor rootfs absent: {} (cannot build a valid OCI bundle)",
            rootfs.display()
        )));
    }
    // CT-007 slice 3, piece 7c: `prep` was already validated (`require_oci_layout_matches_prepared_mode`)
    // by `launch_with` before this function was ever called — `mode` is the single value every
    // downstream use below shares.
    let mode = prep.mode;

    let mut bundle = stage_production_bundle(cfg, rootfs).map_err(RunFailure::uncommitted)?;

    // CT-003b (SI-017): establish the OUT-OF-BAND memory cgroup BEFORE spawning runsc and FAIL
    // CLOSED if it cannot be established (rootless runsc would otherwise run the workload's
    // anonymous memory UNBOUNDED — a host-DoS escape). CT-007 slice 3, piece 7b: creation lives
    // HERE (one call-frame up from `run_and_capture`, which now only borrows it) so this function
    // — not `run_and_capture` — owns the checked teardown via `finalize_runtime`. A creation
    // failure here happens AFTER the bundle was staged, so it must clean the bundle up itself
    // (nothing else will).
    let cgroup = match MemoryCgroup::create(spec.limits.mem_bytes, spec.limits.cpu_millis) {
        Ok(cgroup) => cgroup,
        Err(e) => {
            return Err(RunFailure::uncommitted(match bundle.cleanup() {
                Ok(()) => e,
                Err(cleanup) => format!("{e}; {cleanup}"),
            }));
        }
    };

    // CT-007 slice 3, piece 7c: durably bind the lease BEFORE ever calling `run_and_capture` —
    // immediately after the cgroup exists (so `cgroup.identity()` is available), while nothing has
    // spawned yet. Re-revalidates the runsc-root identity ONE MORE TIME, live, right at this exact
    // boundary (minimizing the gap between "confirmed unchanged" and "durably bound to it") — the
    // earlier read (in `launch_with`, building `PreparedRuntimeMode`) is what this compares against,
    // never a substitute for this check. Sol's round-2 review: the bind-then-capture composition
    // itself (never invoking the capture continuation after a failed/unconfirmed bind) is now
    // `bind_then_continue` — a deterministic seam covering that exact security property with a bare
    // counting closure, with no real runsc spawn involved.
    let timeout = Duration::from_secs(spec.limits.timeout_secs as u64);
    let redaction = spec.resolved_secrets().redaction_plan().clone();
    // The single `run_and_capture` continuation — invoked at most once, by whichever bind path below
    // succeeds (mutually-exclusive match arms, so the moved `launch_permit`/`output` are fine).
    let revalidate = || {
        revalidated_explicit_userns_root_identity()
            .map_err(|reason| format!("runsc-root identity revalidation failed: {reason}"))
    };
    let capture = || {
        run_and_capture(
            bin,
            &bundle,
            container_id,
            timeout,
            spec.limits.mem_bytes,
            RunCaptureOptions {
                stdin: None, // CI/agent jobs receive no stdin (the git-wire path supplies the body).
                stdout_mode: StdoutMode::CappedHead,
                cancellation: cancellation.as_atomic(),
                redaction: redaction.clone(),
                output: output.map(|sink| StreamingOutput { sink }),
            },
            Some(launch_permit),
            mode,
            &cgroup,
        )
    };
    let bind_and_capture_result = match &mut prep.binding {
        // CT-007 slice 5b.3-6c: the checkout workload binds through the session (`Prepared → Bound`)
        // at this exact cgroup boundary, never `lease.bind`. Same never-exec-after-a-failed-bind
        // composition as `bind_then_continue`, but over `bind_prepared_lease_given`.
        RuntimeBinding::EnabledPrepared {
            expected_root_identity,
            lease,
            session,
            bind_state,
        } => {
            let expected_root_identity = *expected_root_identity;
            match bind_prepared_lease_given(
                lease,
                session,
                bind_state,
                expected_root_identity,
                container_id,
                cgroup.identity(),
                revalidate,
            ) {
                Ok(()) => Ok(capture()),
                Err(message) => Err(message),
            }
        }
        // The ordinary compute path — byte-identical behaviour: `Rootless` binds nothing, `Enabled`
        // binds via `lease.bind` (`Allocated → Bound`), through the unchanged `bind_then_continue`.
        binding => {
            let enabled = match binding {
                RuntimeBinding::Rootless => None,
                RuntimeBinding::Enabled {
                    expected_root_identity,
                    context,
                } => Some((
                    &mut context.lease,
                    &mut context.bind_state,
                    *expected_root_identity,
                )),
                RuntimeBinding::EnabledPrepared { .. } => {
                    unreachable!("EnabledPrepared is handled in the arm above")
                }
            };
            bind_then_continue(
                enabled,
                container_id,
                cgroup.identity(),
                revalidate,
                capture,
            )
        }
    };
    let (result, child_retirement) = match bind_and_capture_result {
        Ok(pair) => pair,
        Err(message) => {
            // Never call `run_and_capture` after a bind failure — no exec may ever follow a
            // failed/unconfirmed bind. Checked-quiesce the cgroup nobody will use, and clean up
            // the bundle, before surfacing the failure.
            let cleanup = bundle.cleanup();
            let mut failure = match cgroup.quiesce(RUNTIME_QUIESCE_TIMEOUT) {
                Ok(_) => RunFailure::uncommitted(message),
                Err(e) => RunFailure::uncommitted(format!(
                    "{message} AND cgroup quiescence also failed ({e})"
                )),
            };
            if let Err(cleanup) = cleanup {
                failure = augment_run_failure_message(failure, cleanup);
            }
            return Err(failure);
        }
    };
    let primary: Result<ContainerRun, RunFailure> = match result {
        Ok(outcome) => {
            let result = build_result(spec, &outcome, &redaction);
            match bundle.cleanup() {
                Ok(()) => Ok(ContainerRun {
                    child: Box::new(SpawnedRunsc {
                        bin,
                        container_id: container_id.to_string(),
                        mode,
                    }),
                    bundle_dir: bundle.path.clone(),
                    result,
                    run_error: outcome.stream_error,
                }),
                Err(cleanup) => Err(RunFailure::executed(cleanup, result.usage)),
            }
        }
        Err(e) => {
            // Spawning/waiting failed before a trustworthy result — checked cleanup + surface both.
            Err(match bundle.cleanup() {
                Ok(()) => e,
                Err(cleanup) => augment_run_failure_message(e, cleanup),
            })
        }
    };
    // Checked teardown replaces the old best-effort `delete_container`/`cgroup.cleanup()` pair —
    // ALWAYS runs, regardless of whether the primary run succeeded or failed, and NEVER discards
    // whichever outcome it finds. CT-007 slice 3, piece 7b (Sol's round-2 review, blocker 1): the
    // envelope is returned to `launch_with`, not settled here — `launch_with` (not this function)
    // will own the bound `UserNamespaceLease` starting in 7c, and needs this evidence to decide
    // whether to release it, before ever collapsing this back to a bare `Result`.
    Ok(finalize_and_merge(
        primary,
        bin,
        container_id,
        &prep.prepared_mode,
        cgroup,
        RUNTIME_QUIESCE_TIMEOUT,
        child_retirement,
    ))
}

/// A unique suffix for bundle dirs / container ids. The wall-clock nanos alone are NOT unique: two
/// launches on different threads can read the SAME nanosecond (the clock resolution is coarser than a
/// launch), colliding on the bundle path (`symlink rootfs: File exists`). Mixing in a per-process
/// monotonically-incrementing counter makes the suffix collision-proof WITHIN the process; the nanos
/// keep it unique ACROSS processes.
pub(super) fn unique_suffix() -> u128 {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    // Shift the nanos up and OR in the sequence so both contribute to a unique value.
    (nanos << 24) | (seq & 0xff_ffff)
}

/// RAII ownership of a per-job private OCI bundle. Normal cleanup is checked and can fail the launch;
/// `Drop` is the guaranteed rollback for unwinding/error paths that cannot return another result.
struct BundleCleanupGuard {
    path: PathBuf,
    armed: bool,
}

impl BundleCleanupGuard {
    pub(super) fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    pub(super) fn cleanup(&mut self) -> Result<(), String> {
        if !self.armed {
            return Ok(());
        }
        std::fs::remove_dir_all(&self.path)
            .map_err(|error| format!("bundle dir {:?} removal failed: {error}", self.path))?;
        self.armed = false;
        Ok(())
    }
}

impl Drop for BundleCleanupGuard {
    fn drop(&mut self) {
        if self.armed {
            self.cleanup().unwrap_or_else(|error| {
                // Continuing after a secret-bearing bundle could not be removed is never safe.
                // During an existing unwind this deliberately aborts via a double panic rather
                // than silently persisting the file; on an ordinary drop it makes the failure
                // observable to the caller/test harness.
                panic!("fail-closed secret bundle cleanup: {error}")
            });
        }
    }
}

pub(super) struct StagedProductionBundle {
    pub(super) path: PathBuf,
    cleanup: BundleCleanupGuard,
    // Held across staging, spawn, execution, and teardown so the verified vendor inode stays pinned
    // (not reclaimed/recycled) while runsc may still open the real-path mount source.
    pub(super) _cargo_vendor: Option<FdBoundCargoVendor>,
}

impl StagedProductionBundle {
    pub(super) fn cleanup(&mut self) -> Result<(), String> {
        self.cleanup.cleanup()
    }
}

impl std::ops::Deref for StagedProductionBundle {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

/// Stage a self-contained OCI bundle in a temp dir — the SAME pattern as
/// `escape_drill_gvisor_test::stage_bundle` (no forked recipe): a `rootfs` symlink → the staged
/// minimal rootfs (`runsc` reads `root.path = "rootfs"` relative to the bundle) + the production
/// `config.json` from [`OciConfig::to_json`]. Returns the bundle dir.
pub(super) fn stage_production_bundle(
    cfg: &OciConfig,
    rootfs: &Path,
) -> Result<StagedProductionBundle, String> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

    // Verify and fd-bind before producing any bundle bytes. For a structured build this capability
    // is moved into `StagedProductionBundle` and kept alive through the entire runtime lifecycle.
    let cargo_vendor = cfg.fd_bind_cargo_vendor_before_spawn()?;

    let bundle = std::env::temp_dir().join(format!(
        "myelin-gvisor-prod-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&bundle)
        .map_err(|e| format!("create private bundle dir {bundle:?}: {e}"))?;
    let mut staged = StagedProductionBundle {
        path: bundle.clone(),
        cleanup: BundleCleanupGuard::new(bundle.clone()),
        _cargo_vendor: cargo_vendor,
    };
    #[cfg(unix)]
    if let Err(error) = std::os::unix::fs::symlink(rootfs, bundle.join("rootfs")) {
        let cleanup = staged.cleanup();
        return Err(match cleanup {
            Ok(()) => format!("symlink rootfs into bundle: {error}"),
            Err(cleanup) => format!("symlink rootfs into bundle: {error}; {cleanup}"),
        });
    }
    let cargo_config_path = if cfg.has_cargo_vendor() {
        let path = bundle.join("cargo-config.toml");
        let mut cargo_config = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o444)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) => {
                let cleanup = staged.cleanup();
                return Err(match cleanup {
                    Ok(()) => format!("create server Cargo config: {error}"),
                    Err(cleanup) => {
                        format!("create server Cargo config: {error}; {cleanup}")
                    }
                });
            }
        };
        if let Err(error) = cargo_config
            .write_all(SERVER_CARGO_CONFIG_TOML.as_bytes())
            .and_then(|()| cargo_config.sync_all())
            .and_then(|()| std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)))
        {
            drop(cargo_config);
            let cleanup = staged.cleanup();
            return Err(match cleanup {
                Ok(()) => format!("write server Cargo config: {error}"),
                Err(cleanup) => format!("write server Cargo config: {error}; {cleanup}"),
            });
        }
        drop(cargo_config);
        Some(path)
    } else {
        None
    };
    let config_path = bundle.join("config.json");
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&config_path)
    {
        Ok(file) => file,
        Err(error) => {
            let cleanup = staged.cleanup();
            return Err(match cleanup {
                Ok(()) => format!("create private config.json: {error}"),
                Err(cleanup) => format!("create private config.json: {error}; {cleanup}"),
            });
        }
    };
    let cargo_vendor_source = staged
        ._cargo_vendor
        .as_ref()
        .map(|bound| bound.vendor_mount_source.as_path());
    let json = match cfg
        .to_json_zeroizing_with_cargo_sources(cargo_config_path.as_deref(), cargo_vendor_source)
    {
        Ok(json) => json,
        Err(error) => {
            drop(file);
            let cleanup = staged.cleanup();
            return Err(match cleanup {
                Ok(()) => error,
                Err(cleanup) => format!("{error}; {cleanup}"),
            });
        }
    };
    if let Err(error) = file
        .write_all(json.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let cleanup = staged.cleanup();
        return Err(match cleanup {
            Ok(()) => format!("write private config.json: {error}"),
            Err(cleanup) => format!("write private config.json: {error}; {cleanup}"),
        });
    }
    drop(file);
    Ok(staged)
}

/// Which invocation mode a prepared runtime is committed to, paired ATOMICALLY with whatever
/// identity evidence that mode implies must hold at teardown (CT-007 slice 3, piece 7b, Sol's
/// review point 5) — a caller cannot pass an inconsistent `(RunscInvocationMode, identity)` pair
/// because there is only ever ONE value to pass. 7b only ever constructs `Rootless`; 7c adds the
/// `ExplicitUserNamespace` constructor without reshaping this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreparedRuntimeMode {
    Rootless,
    ExplicitUserNamespace {
        config: UserNamespaceConfig,
        /// The runsc state-root identity [`preflight_explicit_userns_policy`] validated at boot —
        /// re-confirmed via [`revalidated_explicit_userns_root_identity`] at teardown time, so the
        /// SAME proof covers both the launch and the teardown of one run.
        expected_root_identity: (u64, u64),
    },
}

impl PreparedRuntimeMode {
    pub(super) fn invocation_mode(self) -> RunscInvocationMode {
        match self {
            PreparedRuntimeMode::Rootless => RunscInvocationMode::Rootless,
            PreparedRuntimeMode::ExplicitUserNamespace { config, .. } => {
                RunscInvocationMode::ExplicitUserNamespace(config)
            }
        }
    }
}

/// CT-007 slice 3, piece 7b (Sol's round-2 review): `cfg.invocation_mode()` (what actually gets
/// executed and mounted) and a `PreparedRuntimeMode` (what checked deletion/finalization expects)
/// were previously constructed INDEPENDENTLY at every call site — nothing stopped them from
/// disagreeing. This is the ONE place that compares them and hands back the single
/// `RunscInvocationMode` every downstream use (`run_and_capture`, `SpawnedRunsc`,
/// `retire_container`, `finalize_runtime`) must share — called BEFORE any spawn attempt, so a
/// disagreement refuses at zero cost rather than executing under one mode while (dishonestly)
/// finalizing under another.
pub(super) fn require_oci_layout_matches_prepared_mode(
    cfg: &OciConfig,
    prepared_mode: &PreparedRuntimeMode,
) -> Result<RunscInvocationMode, String> {
    let mode = prepared_mode.invocation_mode();
    let oci_mode = cfg.invocation_mode();
    if oci_mode != mode {
        return Err(format!(
            "the OCI config's invocation mode {oci_mode:?} disagrees with the prepared runtime \
             mode {mode:?} — refusing rather than executing under one and finalizing under \
             the other"
        ));
    }
    Ok(mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::user_namespace::UserNamespaceConfig;

    use std::path::PathBuf;

    use crate::gvisor::test_fixtures::*;

    #[test]
    fn production_secret_bundle_is_owner_only_and_drop_cleans_post_stage_error() {
        use std::os::unix::fs::PermissionsExt;

        let rootfs = std::env::temp_dir().join(format!(
            "myelin-secret-bundle-rootfs-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir(&rootfs).unwrap();
        let mut job = spec(vec![]);
        job.secret_refs = vec![crate::SecretRef {
            name: "DEPLOY_TOKEN".into(),
            handle: "myelin://acme/ci/secret/deploy".into(),
        }];
        let job = job
            .with_resolved_secrets(vec![crate::ResolvedSecretEnv::new(
                "DEPLOY_TOKEN",
                "secret-bundle-material",
            )])
            .unwrap();
        let cfg = GvisorBackend::oci_config(&job).unwrap();
        let (bundle_path, post_stage): (PathBuf, Result<(), &'static str>) = {
            let staged = stage_production_bundle(&cfg, &rootfs).unwrap();
            let config_path = staged.path.join("config.json");

            assert_eq!(
                std::fs::metadata(&staged.path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&config_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );

            (
                staged.path.clone(),
                Err("simulated post-stage launch failure"),
            )
        };
        assert_eq!(
            post_stage.unwrap_err(),
            "simulated post-stage launch failure"
        );
        assert!(!bundle_path.exists());
        std::fs::remove_dir_all(rootfs).unwrap();
    }

    #[test]
    fn cargo_vendor_digest_drift_fails_closed_before_spawn_continuation() {
        let fixture = cargo_boundary_fixture("drift");
        let cfg = wired_cargo_config(&fixture);
        std::fs::write(
            fixture.root.join("asset/vendor/itoa-1.0.15/tampered"),
            b"drift",
        )
        .unwrap();
        let error = match stage_production_bundle(&cfg, &fixture.rootfs) {
            Ok(_) => panic!("post-registration asset drift must refuse"),
            Err(error) => error,
        };
        assert!(error.contains("drifted before spawn"), "{error}");
    }

    /// Sol's round-2 review, blocker 2: `cfg.invocation_mode()` (what actually executes) and a
    /// `PreparedRuntimeMode` (what checked deletion/finalization expects) were previously
    /// constructed independently, with nothing refusing a disagreement between them. Proves the
    /// mismatch refuses before any spawn attempt, in both directions.
    #[test]
    fn require_oci_layout_matches_prepared_mode_refuses_a_disagreement() {
        let userns_config = UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005);
        let explicit_userns_cfg = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_user_namespace(userns_config)
            .unwrap();
        let rootless_cfg = GvisorBackend::oci_config(&spec(vec![])).unwrap();

        // The OCI config selected ExplicitUserNamespace, but the prepared mode says Rootless.
        assert!(
            require_oci_layout_matches_prepared_mode(
                &explicit_userns_cfg,
                &PreparedRuntimeMode::Rootless
            )
            .is_err(),
            "an ExplicitUserNamespace OCI config paired with a Rootless prepared mode must refuse"
        );

        // The reverse disagreement: OCI config is Rootless, but the prepared mode says
        // ExplicitUserNamespace.
        assert!(
            require_oci_layout_matches_prepared_mode(
                &rootless_cfg,
                &PreparedRuntimeMode::ExplicitUserNamespace {
                    config: userns_config,
                    expected_root_identity: (1, 2),
                }
            )
            .is_err(),
            "a Rootless OCI config paired with an ExplicitUserNamespace prepared mode must refuse"
        );

        // Agreement in both directions must be accepted.
        assert!(require_oci_layout_matches_prepared_mode(
            &rootless_cfg,
            &PreparedRuntimeMode::Rootless
        )
        .is_ok());
        assert!(require_oci_layout_matches_prepared_mode(
            &explicit_userns_cfg,
            &PreparedRuntimeMode::ExplicitUserNamespace {
                config: userns_config,
                expected_root_identity: (1, 2),
            }
        )
        .is_ok());
    }

    /// CT-007 slice 2's live pinned drill: an `OciConfig` with `ExplicitUserNamespace` actually
    /// boots through the REAL production `stage_production_bundle`/`run_and_capture` machinery
    /// (not a throwaway spike bundle) — proving the exact command-line/OCI-JSON contract this
    /// slice produces is genuinely runnable by the pinned `runsc` build, not merely well-formed
    /// JSON. A real [`crate::user_namespace::UserNamespaceAllocator`] leases the subordinate
    /// uid/gid pair from this host's REAL `/etc/subuid`/`/etc/subgid`. SKIPS gracefully without
    /// `runsc` on PATH, the staged escape-drill rootfs, or a usable subordinate-range entry for
    /// this process's own uid (present on this development host — CI hosts may lack one).
    #[test]
    #[cfg(feature = "integration")]
    fn explicit_user_namespace_boots_through_the_real_production_run_path() {
        // Sol's review, round 8: this drill previously resolved its OWN `bin` via a separate PATH
        // search, while `preflight_explicit_userns_policy` validates the process-global cached
        // `runsc_bin()` — the two could structurally diverge (e.g. if `RESOLVED_RUNSC_BIN` was
        // already initialized to something else earlier in this process), letting the drill
        // validate binary A and then execute binary B. Fixed by removing the drill's own
        // resolution entirely and using `runsc_bin()` — the SAME binary preflight just validated —
        // for the actual launch/delete calls below. `preflight_explicit_userns_policy` already
        // fails (and this drill already skips gracefully) if `runsc_bin()` doesn't resolve to a
        // usable, pinned binary at all, so no separate "runsc not on PATH" precondition check is
        // needed here anymore.
        //
        // This drill's whole point is proving the exact CLI/OCI contract this slice produces is
        // genuinely runnable — that claim is only proven against the SAME runsc release+build it
        // was validated against (Sol's review, round 4), a different (even same-version-string)
        // build is a drill PRECONDITION miss, not a bug this drill exists to catch, and the pin
        // check must never run against an unhardened pathname (Sol's review, round 7: a standalone
        // call to `verify_pinned_explicit_userns_runsc` here, BEFORE hardening, violated that
        // function's own documented caller precondition — a matching binary could still be
        // replaced between the hash and the `--version` exec if the pathname itself were never
        // proven immutable first).
        //
        // `apply_runsc_invocation_policy`'s `ExplicitUserNamespace` branch now REFUSES outright
        // without a validated policy (Sol's review, round 4) — this drill exercises the REAL
        // production activation path, so it must actually install one, exactly as a real
        // production caller would, rather than reaching into `EXPLICIT_USERNS_POLICY` directly.
        if let Err(e) = preflight_explicit_userns_policy(
            resolved_explicit_userns_helper_dir(),
            resolved_explicit_userns_runsc_root(),
        ) {
            eprintln!("[explicit-userns drill] SKIP: preflight_explicit_userns_policy failed: {e}");
            return;
        }
        let bin = runsc_bin();
        let rootfs = crate::resolved_gvisor_rootfs();
        if !rootfs.exists() {
            eprintln!("[explicit-userns drill] SKIP: staged rootfs absent at {rootfs:?}");
            return;
        }
        let leases_dir = std::env::temp_dir().join(format!(
            "myelin-userns-drill-leases-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        // `try_new_for_tests` (not the strict production `try_new`): this drill's `leases_dir`
        // sits under `std::env::temp_dir()`, whose PARENT (`/tmp`) is world-writable on virtually
        // every host — the strict production constructor's parent-not-writable-by-us check (Sol's
        // review, closing the replaceable-lock-anchor gap) would always refuse it here, which
        // would test nothing about this drill's actual purpose (proving the REAL bundle/launch
        // path boots an explicit-userns container). That deployment-layout requirement belongs to
        // slice 4's production-activation drills, which verify the REAL runner deployment's
        // directory permissions — not this slice's own test suite. The REAL host `/etc/subuid`/
        // `/etc/subgid` are still used (this host's copies are already root-owned, mode 644).
        let allocator = match crate::user_namespace::UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            Path::new("/etc/subuid"),
            Path::new("/etc/subgid"),
            1,
            Arc::new(|msg: &str| eprintln!("[explicit-userns drill incident] {msg}")),
        ) {
            Ok(a) => a,
            Err(
                e @ crate::user_namespace::UserNamespaceAllocatorError::NoSubordinateEntry {
                    ..
                },
            ) => {
                eprintln!(
                    "[explicit-userns drill] SKIP: no usable /etc/subuid|subgid range for this \
                     process's uid: {e}"
                );
                let _ = std::fs::remove_dir_all(&leases_dir);
                return;
            }
            Err(e) => panic!(
                "allocator construction failed with an unexpected (non-\"no usable range\") \
                 error — this indicates a real bug (malformed/unsafe config, lock contention, \
                 corrupt state, unsafe directory), not an absent host configuration: {e}"
            ),
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");

        let mut command_spec = spec(vec![]);
        command_spec.command = vec!["/bin/sh".into(), "-c".into(), "id".into()];
        let profile = HardeningProfile::derive(&command_spec);
        let cfg = OciConfig::from_spec(&command_spec, &profile)
            .with_user_namespace(lease.config())
            .expect("a fresh Rootless config must accept a user-namespace layout selection");
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::ExplicitUserNamespace(lease.config())
        );

        let bundle = stage_production_bundle(&cfg, &rootfs).expect("stage the production bundle");
        let container_id = format!(
            "myelin-userns-drill-{}-{}",
            std::process::id(),
            unique_suffix()
        );
        // CT-007 slice 3, piece 7b: `preflight_explicit_userns_policy` above already installed the
        // REAL global policy this drill is exercising — the `(0, 0)` placeholder this test
        // previously used for `runsc_root_identity` is gone now that piece 7b's real
        // `finalize_runtime`/`revalidated_explicit_userns_root_identity` wiring exists; this is the
        // SAME identity `finalize_runtime` will re-confirm at teardown below.
        let runsc_root_identity = revalidated_explicit_userns_root_identity()
            .expect("the policy this drill just installed via preflight must revalidate cleanly");
        let cgroup = MemoryCgroup::create(
            command_spec.limits.mem_bytes,
            command_spec.limits.cpu_millis,
        )
        .expect("establish a real memory cgroup for this drill");
        let cgroup_identity = cgroup.identity();
        lease
            .bind(container_id.clone(), runsc_root_identity, cgroup_identity)
            .expect("bind must succeed for a fresh Allocated lease");
        // Sol's round-3 review: construct `prepared_mode` and derive `mode` through the SAME
        // agreement-checking helper the production path uses, rather than reading
        // `cfg.invocation_mode()` independently — this drill should demonstrate the exact contract
        // it exercises, not bypass it.
        let prepared_mode = PreparedRuntimeMode::ExplicitUserNamespace {
            config: lease.config(),
            expected_root_identity: runsc_root_identity,
        };
        let mode = require_oci_layout_matches_prepared_mode(&cfg, &prepared_mode)
            .expect("the drill's own cfg and prepared mode must agree");
        let (result, child_retirement) = run_and_capture(
            bin,
            &bundle,
            &container_id,
            Duration::from_secs(10),
            command_spec.limits.mem_bytes,
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
        let evidence = finalize_runtime(
            bin,
            &container_id,
            &prepared_mode,
            cgroup,
            RUNTIME_QUIESCE_TIMEOUT,
            child_retirement,
        )
        .expect("checked teardown must succeed through the real production path");
        assert_eq!(
            evidence.namespace,
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity
            }
        );
        let _ = std::fs::remove_dir_all(&bundle);

        let outcome = result.unwrap_or_else(|e| {
            panic!("run_and_capture must succeed through the real production path: {e:?}")
        });
        assert!(
            !outcome.timed_out,
            "the guest `id` command must not time out"
        );
        assert_eq!(
            outcome.exit,
            Some(0),
            "the guest `id` command must exit 0, stderr: {}",
            String::from_utf8_lossy(&outcome.stderr)
        );
        let stdout = String::from_utf8_lossy(&outcome.stdout);
        assert!(
            stdout.contains("uid=65534") && stdout.contains("gid=65534"),
            "the guest must report uid/gid 65534 (mapped via the OCI uidMappings/gidMappings \
             this slice emits), got: {stdout:?}"
        );

        let nonce = lease.nonce_for_tests();
        lease
            .release(
                crate::user_namespace::UserNamespaceQuiescenceProof::assert_for_tests(
                    nonce,
                    container_id,
                    runsc_root_identity,
                    cgroup_identity,
                ),
            )
            .expect("release with the lease's own nonce and bound identity must succeed");
        let _ = std::fs::remove_dir_all(&leases_dir);
    }
}

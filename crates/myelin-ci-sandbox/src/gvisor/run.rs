use super::*;
use crate::user_namespace::{RunscInvocationMode, UserNamespaceConfig};
use crate::{JobSpec, LaunchPermit, SandboxCancellation, SandboxOutputSink};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

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
    if !rootfs.exists() {
        return Err(RunFailure::uncommitted(format!(
            "staged gVisor rootfs absent: {} (cannot build a valid OCI bundle)",
            rootfs.display()
        )));
    }
    let mode = prep.mode;

    let mut bundle = stage_production_bundle(cfg, rootfs).map_err(RunFailure::uncommitted)?;

    let cgroup = match MemoryCgroup::create(spec.limits.mem_bytes, spec.limits.cpu_millis) {
        Ok(cgroup) => cgroup,
        Err(e) => {
            return Err(RunFailure::uncommitted(match bundle.cleanup() {
                Ok(()) => e,
                Err(cleanup) => format!("{e}; {cleanup}"),
            }));
        }
    };

    let timeout = Duration::from_secs(spec.limits.timeout_secs as u64);
    let redaction = spec.resolved_secrets().redaction_plan().clone();
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
                stdin: None,
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
        Err(e) => Err(match bundle.cleanup() {
            Ok(()) => e,
            Err(cleanup) => augment_run_failure_message(e, cleanup),
        }),
    };
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

pub(super) fn unique_suffix() -> u128 {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    (nanos << 24) | (seq & 0xff_ffff)
}

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
            self.cleanup()
                .unwrap_or_else(|error| panic!("fail-closed secret bundle cleanup: {error}"));
        }
    }
}

pub(super) struct StagedProductionBundle {
    pub(super) path: PathBuf,
    cleanup: BundleCleanupGuard,
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

pub(super) fn stage_production_bundle(
    cfg: &OciConfig,
    rootfs: &Path,
) -> Result<StagedProductionBundle, String> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreparedRuntimeMode {
    Rootless,
    ExplicitUserNamespace {
        config: UserNamespaceConfig,
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

pub(super) fn require_oci_layout_matches_prepared_mode(
    cfg: &OciConfig,
    prepared_mode: &PreparedRuntimeMode,
) -> Result<RunscInvocationMode, String> {
    let mode = prepared_mode.invocation_mode();
    let oci_mode = cfg.invocation_mode();
    if oci_mode != mode {
        return Err(format!(
            "the OCI config's invocation mode {oci_mode:?} disagrees with the prepared runtime \
             mode {mode:?} - refusing rather than executing under one and finalizing under \
             the other"
        ));
    }
    Ok(mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "integration")]
    use crate::hardening::HardeningProfile;
    #[cfg(feature = "integration")]
    use crate::redaction::RedactionPlan;
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

    #[test]
    fn require_oci_layout_matches_prepared_mode_refuses_a_disagreement() {
        let userns_config = UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005);
        let explicit_userns_cfg = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_user_namespace(userns_config)
            .unwrap();
        let rootless_cfg = GvisorBackend::oci_config(&spec(vec![])).unwrap();

        assert!(
            require_oci_layout_matches_prepared_mode(
                &explicit_userns_cfg,
                &PreparedRuntimeMode::Rootless
            )
            .is_err(),
            "an ExplicitUserNamespace OCI config paired with a Rootless prepared mode must refuse"
        );

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

    #[test]
    #[cfg(feature = "integration")]
    fn explicit_user_namespace_boots_through_the_real_production_run_path() {
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
                 error - this indicates a real bug (malformed/unsafe config, lock contention, \
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
        let bundle_path = bundle.path.clone();
        drop(bundle);
        assert!(!bundle_path.exists());

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

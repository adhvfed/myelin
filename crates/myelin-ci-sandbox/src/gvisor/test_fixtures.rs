//! Shared fixtures for the `gvisor` submodules' unit tests.

use super::*;
use crate::gvisor::checkout_transport_test_support::FakeRunsc;
use crate::hardening::HardeningProfile;
use crate::runner::PreparationPhase;
#[cfg(feature = "test-support")]
use crate::user_namespace::UserNamespaceAllocator;
use crate::user_namespace::UserNamespaceConfig;
#[cfg(feature = "test-support")]
use crate::workspace_manager::WorkspaceManager;
#[cfg(feature = "test-support")]
use crate::workspace_manager::WorkspaceStorageMode;
use crate::EnvVar;
use crate::{
    CompletionSettlementOwner, EgressPolicy, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget,
    ReserveHandle, ResourceLimits, ResourceUsage, RunTokenCredential, RunnerHooks,
    SandboxOutputSink, SandboxOutputStream, SandboxResult, TrustTier, WorkspaceSpec,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
pub(super) struct RecordingOutput {
    pub(super) bytes: Mutex<Vec<u8>>,
}

impl SandboxOutputSink for RecordingOutput {
    fn emit(&self, _stream: SandboxOutputStream, frame: &[u8]) -> Result<(), String> {
        self.bytes.lock().unwrap().extend_from_slice(frame);
        Ok(())
    }
}

/// A fresh, empty temp directory for a head/`Cargo.lock` verification fixture.
pub(super) fn temp_dir_for(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "myelin-checkout-headcheck-{name}-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A real, on-disk, empty fixture rootfs — hashed with the SAME pure-Rust
/// [`crate::canonical_tar::canonical_tree_sha256_hex`] the registry itself uses — so [`spec`]'s
/// image is a GENUINELY verifiable pin, not a fabricated placeholder digest a real registry
/// lookup could never match. Shared (same fixed path) across every test in this module — they
/// only ever READ it (construction-time hashing happens once, in [`test_registry`]), never
/// mutate it, so sharing across parallel test threads within this one process is safe.
pub(super) fn fixture_rootfs_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "myelin-gvisor-unit-test-fixture-rootfs-{}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::create_dir(dir.join("workspace"));
    dir
}

/// The digest-pinned [`ImageRef`] matching [`fixture_rootfs_dir`]'s REAL current content.
pub(super) fn fixture_image() -> ImageRef {
    let digest = crate::canonical_tar::canonical_tree_sha256_hex(&fixture_rootfs_dir())
        .expect("hash the fixture rootfs dir");
    ImageRef::pinned(format!("test.local/fixture-rootfs@sha256:{digest}")).unwrap()
}

/// A registry mapping [`fixture_image`] to [`fixture_rootfs_dir`] — the registry every unit test
/// in this module that calls `launch_with`/`launch` constructs its [`GvisorBackend`] with. These
/// tests never run a real `runsc` (they inject a fake `run` closure), so all that matters is that
/// construction genuinely verifies (once) before the fake closure runs.
pub(super) fn test_registry() -> Arc<crate::asset_registry::GvisorAssetRegistry> {
    Arc::new(
        crate::asset_registry::GvisorAssetRegistry::from_bindings(vec![
            crate::asset_registry::RootfsAssetBinding {
                image: fixture_image(),
                rootfs: fixture_rootfs_dir(),
            },
        ])
        .expect("fixture binding verifies"),
    )
}

pub(super) fn spec(allow: Vec<String>) -> JobSpec {
    JobSpec::new(
        JobKind::Agent,
        fixture_image(),
        vec!["python3".into(), "-c".into(), "print(1)".into()],
        vec![],
        vec![],
        EgressPolicy { allow },
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 64,
            timeout_secs: 120,
        },
        WorkspaceSpec::default(),
        TrustTier::UntrustedFork,
        RunTokenCredential::new("test-bearer", "j", 300).unwrap(),
        MeterTarget {
            reserve_id: "r".into(),
        },
        IdemToken("idem-runsc-1".into()),
    )
    .unwrap()
}

pub(super) struct CargoBoundaryFixture {
    pub(super) root: PathBuf,
    pub(super) rootfs: PathBuf,
    pub(super) reference: ImageRef,
    lock_sha256: String,
    pub(super) registry: crate::asset_registry::GvisorAssetRegistry,
}

impl Drop for CargoBoundaryFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub(super) fn cargo_boundary_fixture(tag: &str) -> CargoBoundaryFixture {
    let root = std::env::temp_dir().join(format!(
        "myelin-cargo-boundary-{tag}-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let vendor_asset = root.join("asset");
    let rootfs = root.join("rootfs");
    std::fs::create_dir_all(vendor_asset.join("vendor/itoa-1.0.15")).unwrap();
    std::fs::create_dir_all(vendor_asset.join(".cargo")).unwrap();
    std::fs::create_dir_all(rootfs.join("opt/myelin/cargo-vendor")).unwrap();
    std::fs::write(
        vendor_asset.join("vendor/itoa-1.0.15/.cargo-checksum.json"),
        b"{}",
    )
    .unwrap();
    std::fs::write(
        vendor_asset.join("vendor/itoa-1.0.15/lib.rs"),
        b"pub fn fixture() {}",
    )
    .unwrap();
    std::fs::write(
        vendor_asset.join(".cargo/config.toml"),
        SERVER_CARGO_CONFIG_TOML,
    )
    .unwrap();
    std::fs::write(vendor_asset.join("Cargo.lock"), b"fixture lock\n").unwrap();
    let lock_sha256 =
        crate::asset_registry::file_sha256_hex(&vendor_asset.join("Cargo.lock")).unwrap();
    let digest = crate::canonical_tar::canonical_tree_sha256_hex(&vendor_asset).unwrap();
    let reference =
        ImageRef::pinned(format!("test.local/cargo-vendor-{tag}@sha256:{digest}")).unwrap();
    let registry = crate::asset_registry::GvisorAssetRegistry::from_bindings_with_cargo_vendor(
        Vec::new(),
        vec![crate::asset_registry::CargoVendorAssetBinding {
            reference: reference.clone(),
            root: vendor_asset,
            cargo_lock_sha256: lock_sha256.clone(),
        }],
    )
    .unwrap();
    CargoBoundaryFixture {
        root,
        rootfs,
        reference,
        lock_sha256,
        registry,
    }
}

pub(super) fn structured_cargo_spec(reference: &ImageRef) -> JobSpec {
    JobSpec::new(
        JobKind::Ci,
        fixture_image(),
        vec![
            "cargo".into(),
            "build".into(),
            "--locked".into(),
            "--config".into(),
            CARGO_SOURCE_REPLACE_CONFIG.into(),
            "--config".into(),
            CARGO_VENDOR_DIRECTORY_CONFIG.into(),
        ],
        vec![
            EnvVar {
                name: "CARGO_HOME".into(),
                value: STRUCTURED_CARGO_HOME.into(),
            },
            EnvVar {
                name: "CARGO_NET_OFFLINE".into(),
                value: "true".into(),
            },
            EnvVar {
                name: CARGO_SOURCE_REPLACE_ENV.into(),
                value: CARGO_VENDOR_SOURCE_NAME.into(),
            },
            EnvVar {
                name: CARGO_VENDOR_DIRECTORY_ENV.into(),
                value: OCI_CARGO_VENDOR_MOUNT.into(),
            },
            EnvVar {
                name: ENV_CARGO_VENDOR_ASSET.into(),
                value: reference.reference.clone(),
            },
        ],
        vec![],
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 64 << 20,
            pids_max: 64,
            timeout_secs: 120,
        },
        WorkspaceSpec::default(),
        TrustTier::UntrustedFork,
        RunTokenCredential::new("test-bearer", "cargo-boundary", 300).unwrap(),
        MeterTarget {
            reserve_id: "r".into(),
        },
        IdemToken("idem-cargo-boundary".into()),
    )
    .unwrap()
}

pub(super) fn wired_cargo_config(fixture: &CargoBoundaryFixture) -> OciConfig {
    let job = structured_cargo_spec(&fixture.reference);
    let profile = HardeningProfile::derive(&job);
    let vendor = selected_cargo_vendor(&job, &fixture.registry)
        .unwrap()
        .expect("structured Cargo selector resolves the registered asset");
    let mut cfg = OciConfig::from_spec(&job, &profile)
        .with_explicit_user_namespace_and_workspace(
            UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005),
            OciWorkspaceMount::for_tests(PathBuf::from("/host/workspace")),
            fixture.rootfs.clone(),
        )
        .unwrap()
        .with_cargo_vendor(vendor)
        .unwrap();
    cfg.bind_materialized_cargo_lock(&fixture.lock_sha256)
        .unwrap();
    cfg
}

pub(super) fn cargo_compute_registry(
    fixture: &CargoBoundaryFixture,
) -> Arc<crate::asset_registry::GvisorAssetRegistry> {
    Arc::new(
        crate::asset_registry::GvisorAssetRegistry::from_bindings_with_cargo_vendor(
            vec![crate::asset_registry::RootfsAssetBinding {
                image: fixture_image(),
                rootfs: fixture_rootfs_dir(),
            }],
            vec![crate::asset_registry::CargoVendorAssetBinding {
                reference: fixture.reference.clone(),
                root: fixture.root.join("asset"),
                cargo_lock_sha256: fixture.lock_sha256.clone(),
            }],
        )
        .unwrap(),
    )
}

/// Writes a real, valid `subuid`/`subgid`-format file naming the CURRENT effective uid, with
/// the given range, so tests never depend on this host's REAL `/etc/subuid`/`/etc/subgid`
/// having an entry for this uid (mirroring `user_namespace.rs`'s own test helper of the same
/// name/shape — Sol's round-2 review: relying on the real files left these tests conditionally
/// skippable on any CI host lacking subordinate-id configuration, exactly the host dependency
/// the builder seam exists to remove).
pub(super) fn write_subordinate_file(path: &Path, start: u32, count: u32) {
    let uid = unsafe { libc::geteuid() };
    std::fs::write(path, format!("{uid}:{start}:{count}\n")).unwrap();
}

pub(super) fn ok_hooks() -> RunnerHooks {
    RunnerHooks::new(
        CompletionSettlementOwner::Hook,
        Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
        Box::new(|_spec, _h, _u| Ok(())),
        Box::new(|_t| Ok(())),
        Box::new(|_s| Ok(())),
    )
}

pub(super) fn outcome(stdout: &[u8], stderr: &[u8]) -> RunscOutcome {
    RunscOutcome {
        exit: Some(0),
        timed_out: false,
        stdout: stdout.to_vec(),
        stdout_truncated: false,
        stderr: stderr.to_vec(),
        wall: Duration::from_secs(1),
        cpu_seconds: Some(1),
        stream_error: None,
    }
}

/// A canned [`ContainerRun`] for the fake path (no real `runsc`): a clean exit-0 result + a fake
/// child + a non-existent bundle dir (its removal on teardown is a harmless no-op).
pub(super) fn fake_run() -> ContainerRun {
    ContainerRun {
        child: Box::new(FakeRunsc),
        bundle_dir: std::env::temp_dir().join("myelin-gvisor-fake-bundle-does-not-exist"),
        result: SandboxResult::stub_ok(ResourceUsage {
            cpu_seconds: 1,
            mem_byte_seconds: 1,
        }),
        run_error: None,
    }
}

/// CT-007 slice 3, piece 7b: since `launch_with`'s `F` now returns a
/// `RuntimeFinalization<Result<ContainerRun, RunFailure>>` envelope (not a bare
/// `Result<ContainerRun, RunFailure>`), every fake test closure that used to return
/// `Ok(fake_run())` needs a fabricated, already-`Finalized` envelope instead — these tests are
/// exercising `launch_with`'s OWN dispatch logic (settle/reserve/hooks), not
/// `finalize_runtime`'s teardown checks, so a canned `Rootless` evidence is all that's needed.
pub(super) fn fake_finalization() -> RuntimeFinalization<Result<ContainerRun, RunFailure>> {
    RuntimeFinalization::Finalized(FinalizedRun {
        primary: Ok(fake_run()),
        evidence: RuntimeQuiescenceEvidence {
            container_id: "fake-container".to_string(),
            namespace: RuntimeNamespaceQuiescence::Rootless,
            cgroup: CgroupQuiescenceEvidence::assert_for_tests((0, 0)),
        },
    })
}

// ───────── CT-007 slice 3, piece 7c: acquire/cleanup/settle Enabled matrix ─────────
//
// These tests use REAL (not fabricated) `WorkspaceManager`/`UserNamespaceAllocator` instances
// — `ManagedWorkspace`/`CapacityLease`/`UserNamespaceLease` have no `#[cfg(test)]` bare
// constructors (deliberately: minting one for real is the whole point of the capability
// boundary), so exercising `acquire_enabled_workspace`/`cleanup_pre_bind_failure`/
// `settle_enabled_workspace_and_lease` at all requires real objects. Unlike
// `explicit_user_namespace_boots_through_the_real_production_run_path`, these do NOT depend on
// `preflight_explicit_userns_policy` (the runsc-root/binary hardening check that skips on this
// development host) — only on real Btrfs (this host has it) and a usable `/etc/subuid`/
// `/etc/subgid` range (also present here) — so they are NOT expected to skip on this host.

/// A real `WorkspaceManager` (open/lock/boot-reconciliation only — NO `create_workspace` call,
/// so no `CAP_SYS_ADMIN`/qgroup privilege is required). Sufficient for tests that never reach
/// past capacity acquisition (e.g. an exhausted-ceiling refusal).
#[cfg(feature = "test-support")]
pub(super) fn real_workspace_manager_without_qgroup_probe_for_tests(
    tag: &str,
) -> Option<(WorkspaceManager, PathBuf)> {
    // `std::env::temp_dir()` (`/tmp`) is frequently a separate tmpfs mount, not Btrfs — use a
    // `$HOME`-rooted path instead, matching `workspace_manager.rs`'s own `btrfs_test_base`.
    let mut base = std::env::home_dir().expect("HOME must be set for this test");
    base.push(format!(
        ".local/state/myelin-gvisor-piece7c-workspace-{tag}-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let sink: crate::workspace_manager::IncidentSink =
        Arc::new(|msg: &str| eprintln!("[piece7c workspace incident] {msg}"));
    match WorkspaceManager::try_new(
        WorkspaceStorageMode::EphemeralDisk {
            base_dir: base.clone(),
            host_capacity_bytes: 1 << 30,
        },
        sink,
    ) {
        Ok(manager) => Some((manager, base)),
        Err(e) => {
            eprintln!(
                "[piece7c] SKIP: no real Btrfs+quota EphemeralDisk support on this host: {e}"
            );
            None
        }
    }
}

/// A real `WorkspaceManager`, additionally probed for the `CAP_SYS_ADMIN` privilege every real
/// `create_workspace` call needs (`btrfs qgroup limit`) — mirrors `workspace_manager.rs`'s own
/// `ephemeral_disk_available` gate. Use this (not the qgroup-probe-free variant above) for any
/// test that actually calls `acquire_enabled_workspace`/`create_workspace` to completion.
#[cfg(feature = "test-support")]
pub(super) fn real_workspace_manager_for_tests(tag: &str) -> Option<(WorkspaceManager, PathBuf)> {
    let mut base = std::env::home_dir().expect("HOME must be set for this test");
    base.push(format!(
        ".local/state/myelin-gvisor-piece7c-workspace-{tag}-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&base).ok()?;
    match crate::workspace_storage::probe_qgroup_privilege(&base) {
        Ok(true) => {}
        Ok(false) => {
            eprintln!(
                "[piece7c] SKIP: this test process lacks CAP_SYS_ADMIN for qgroup operations"
            );
            let _ = std::fs::remove_dir_all(&base);
            return None;
        }
        Err(e) => {
            eprintln!("[piece7c] SKIP: no real Btrfs+quota support on this host: {e}");
            let _ = std::fs::remove_dir_all(&base);
            return None;
        }
    }
    let sink: crate::workspace_manager::IncidentSink =
        Arc::new(|msg: &str| eprintln!("[piece7c workspace incident] {msg}"));
    match WorkspaceManager::try_new(
        WorkspaceStorageMode::EphemeralDisk {
            base_dir: base.clone(),
            host_capacity_bytes: 1 << 30,
        },
        sink,
    ) {
        Ok(manager) => Some((manager, base)),
        Err(e) => {
            eprintln!(
                "[piece7c] SKIP: no real Btrfs+quota EphemeralDisk support on this host: {e}"
            );
            None
        }
    }
}

#[cfg(feature = "test-support")]
pub(super) fn real_userns_allocator_for_tests(
    tag: &str,
) -> Option<(UserNamespaceAllocator, PathBuf)> {
    let leases_dir = std::env::temp_dir().join(format!(
        "myelin-gvisor-piece7c-leases-{tag}-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    match UserNamespaceAllocator::try_new_for_tests(
        leases_dir.clone(),
        Path::new("/etc/subuid"),
        Path::new("/etc/subgid"),
        1,
        Arc::new(|msg: &str| eprintln!("[piece7c userns incident] {msg}")),
    ) {
        Ok(allocator) => Some((allocator, leases_dir)),
        Err(e) => {
            eprintln!(
                "[piece7c] SKIP: no usable /etc/subuid|subgid range for this process's uid: {e}"
            );
            None
        }
    }
}

// ───────── CT-007 slice 5b.3-6c: parent-attempt reservation mode (always-run) ─────────

/// A minimal recording fake [`AttemptAuthority`] for the 6c continuation/orchestrator tests, with
/// optional injected failures for the post-acquisition authority-failure matrix.
pub(super) struct FakeAttemptAuthority {
    pub(super) ops: Mutex<Vec<String>>,
    should_requeue: bool,
    fail_begin_phase: bool,
    fail_mint_phase: bool,
}

#[allow(dead_code)]
impl FakeAttemptAuthority {
    pub(super) fn new(should_requeue: bool) -> Self {
        Self {
            ops: Mutex::new(Vec::new()),
            should_requeue,
            fail_begin_phase: false,
            fail_mint_phase: false,
        }
    }
    pub(super) fn failing_begin_phase() -> Self {
        Self {
            fail_begin_phase: true,
            ..Self::new(true)
        }
    }
    pub(super) fn failing_mint_phase() -> Self {
        Self {
            fail_mint_phase: true,
            ..Self::new(true)
        }
    }
}

impl crate::checkout_orchestration::AttemptAuthority for FakeAttemptAuthority {
    fn begin_phase(
        &self,
        phase: PreparationPhase,
    ) -> Result<(), crate::checkout_orchestration::AttemptAuthorityError> {
        self.ops.lock().unwrap().push(format!("begin:{phase:?}"));
        if self.fail_begin_phase {
            return Err(crate::checkout_orchestration::AttemptAuthorityError(
                "injected begin_phase failure".to_string(),
            ));
        }
        Ok(())
    }
    fn complete_phase(
        &self,
        phase: PreparationPhase,
        usage: ResourceUsage,
    ) -> Result<(), crate::checkout_orchestration::AttemptAuthorityError> {
        self.ops
            .lock()
            .unwrap()
            .push(format!("complete:{phase:?}:{}", usage.cpu_seconds));
        Ok(())
    }
    fn seal_phase(
        &self,
        phase: PreparationPhase,
    ) -> Result<(), crate::checkout_orchestration::AttemptAuthorityError> {
        self.ops.lock().unwrap().push(format!("seal:{phase:?}"));
        Ok(())
    }
    fn renew_preparation_lease(&self) -> Result<(), crate::runner::PreparationLeaseLost> {
        self.ops.lock().unwrap().push("renew".to_string());
        Ok(())
    }
    fn mint_phase_credential(
        &self,
        phase: crate::CheckoutPhase,
    ) -> Result<
        crate::checkout_orchestration::PhaseCredentialCarrier,
        crate::checkout_orchestration::AttemptAuthorityError,
    > {
        self.ops.lock().unwrap().push(format!("mint:{phase:?}"));
        if self.fail_mint_phase {
            return Err(crate::checkout_orchestration::AttemptAuthorityError(
                "injected mint_phase_credential failure".to_string(),
            ));
        }
        Ok(crate::checkout_orchestration::PhaseCredentialCarrier::new(
            RunTokenCredential::new("bearer", format!("jti-{phase:?}"), 300).unwrap(),
            fake_authorization_context(),
            format!("gen-{phase:?}"),
        ))
    }
    fn mint_workload_credential(
        &self,
    ) -> Result<
        crate::checkout_orchestration::WorkloadCredentialCarrier,
        crate::checkout_orchestration::AttemptAuthorityError,
    > {
        self.ops.lock().unwrap().push("mint:Workload".to_string());
        Ok(
            crate::checkout_orchestration::WorkloadCredentialCarrier::new(
                RunTokenCredential::new("bearer", "jti-Workload", 300).unwrap(),
                fake_authorization_context(),
                "gen-Workload",
            ),
        )
    }
    fn should_requeue(&self) -> bool {
        self.should_requeue
    }
}

/// A well-formed preparation reporting identity for the routing tests (CT-007 5b.3-6d STEP 4). The
/// dormant orchestrator/continuation carry it UNCHANGED into any preparation outcome.
pub(super) fn report_claim() -> crate::runner::PreparationReportClaim {
    crate::runner::PreparationReportClaim {
        tenant_id: "acme".to_string(),
        region: "fr-par".to_string(),
        project_id: "00000000-0000-0000-0000-000000000001".to_string(),
        wf_run_id: "11111111-1111-1111-1111-111111111111".to_string(),
        ci_run_id: "44444444-4444-4444-4444-444444444444".to_string(),
        job_id: "22222222-2222-2222-2222-222222222222".to_string(),
        token_authority_handle: "tah-xyz".to_string(),
        idem_token: "11111111-1111-1111-1111-111111111111/build".to_string(),
        lease_owner: "worker-1".to_string(),
        lease_epoch: 7,
        claim_nonce: "33333333-3333-3333-3333-333333333333".to_string(),
        claim_started_at_epoch_secs: 1_000,
        claim_expires_at_epoch_secs: 1_300,
    }
}

pub(super) fn fake_authorization_context() -> crate::RunTokenAuthorizationContext {
    crate::RunTokenAuthorizationContext::CiJob(crate::CiJobAuthorizationContext {
        tenant_id: "acme".to_string(),
        region: "fr-par".to_string(),
        principal_id: "p".to_string(),
        project_id: "00000000-0000-0000-0000-000000000001".to_string(),
        wf_run_id: "wf".to_string(),
        job_id: "j".to_string(),
        lease_owner: "o".to_string(),
        lease_epoch: 1,
        claim_nonce: "n".to_string(),
        claim_started_at_epoch_secs: 0,
        claim_expires_at_epoch_secs: 1,
        reserve_id: "r".to_string(),
        required_capabilities: vec![],
        checkout_scope: None,
        credential_binding: None,
    })
}

// ───────── CT-007 slice 5b.3-6a: the dispose_checkout_runtime BEHAVIORAL cleanup matrix ─────────
//
// Sol's r1 blocker 6: the pure `checkout_cleanup_plan` pin (above, always-run) proves the
// disposition→action MAPPING; these privileged end-to-end tests prove its EXECUTION against a
// REAL workspace+lease — after each disposition, the durable allocator/workspace state is exactly
// what the plan promises (a released slot is REUSABLE; a quarantined slot is NOT reissued and the
// workspace manager is poisoned for operator reconciliation). Gated on real Btrfs+qgroup and a
// usable subuid range, like the acquire/settle matrix above; the pool size is 1, so "the slot was
// released" is observable as "a second `lease()` now succeeds", and "quarantined" as "it fails".

/// A checkout-bearing [`JobSpec`] whose workspace derives a valid [`CheckoutAuthorizationScope`]
/// (`myelin://acme/git/repo/widgets` @ a 40-hex commit), so `AcquiredCheckoutRuntime::acquire`
/// reaches a real acquisition.
#[cfg(feature = "test-support")]
pub(super) fn checkout_spec() -> JobSpec {
    JobSpec::new(
        JobKind::Ci,
        fixture_image(),
        vec!["true".into()],
        vec![],
        vec![],
        EgressPolicy { allow: vec![] },
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 64,
            timeout_secs: 120,
        },
        WorkspaceSpec {
            repo_ref: Some("myelin://acme/git/repo/widgets".to_string()),
            commit: Some("a".repeat(40)),
        },
        TrustTier::UntrustedFork,
        RunTokenCredential::new("test-bearer", "j", 300).unwrap(),
        MeterTarget {
            reserve_id: "r".into(),
        },
        IdemToken("idem-checkout-6a".into()),
    )
    .unwrap()
}

/// Acquire a REAL capsule against fresh real managers (pool size 1). Returns the capsule plus the
/// managers/dirs so the caller can dispose and then probe durable state. `None` = soft skip.
#[cfg(feature = "test-support")]
#[allow(clippy::type_complexity)]
pub(super) fn acquire_real_checkout_capsule(
    tag: &str,
) -> Option<(
    AcquiredCheckoutRuntime,
    WorkspaceManager,
    PathBuf,
    UserNamespaceAllocator,
    PathBuf,
)> {
    let (workspace_manager, workspace_base) = real_workspace_manager_for_tests(tag)?;
    let Some((userns_allocator, leases_dir)) = real_userns_allocator_for_tests(tag) else {
        let _ = std::fs::remove_dir_all(&workspace_base);
        return None;
    };
    let spec = checkout_spec();
    let profile = HardeningProfile::derive(&spec);
    let runtime = AcquiredCheckoutRuntime::acquire(
        &spec,
        &profile,
        PathBuf::from("/abs/staged-rootfs"),
        &workspace_manager,
        &userns_allocator,
        None,
    )
    .expect("acquisition must succeed against a healthy real manager/allocator");
    // Sanity: the acquisition already exhausted the size-1 pool.
    assert!(
        userns_allocator.lease().is_err(),
        "the size-1 pool is exhausted while the capsule holds its lease"
    );
    Some((
        runtime,
        workspace_manager,
        workspace_base,
        userns_allocator,
        leases_dir,
    ))
}

/// A registry mapping a fresh digest-pinned [`ImageRef`] to the REAL staged rootfs
/// [`crate::resolved_gvisor_rootfs`] uses — so a spec naming this image resolves, through the
/// real registry lookup `GvisorBackend::launch_with` performs, to the exact rootfs the drill
/// above already proves is runnable.
#[cfg(feature = "integration")]
pub(super) fn real_userns_drill_registry(
    rootfs: &Path,
) -> Arc<crate::asset_registry::GvisorAssetRegistry> {
    let digest = crate::canonical_tar::canonical_tree_sha256_hex(rootfs)
        .expect("hash the real staged rootfs");
    let image = ImageRef::pinned(format!("test.local/userns-drill@sha256:{digest}")).unwrap();
    Arc::new(
        crate::asset_registry::GvisorAssetRegistry::from_bindings(vec![
            crate::asset_registry::RootfsAssetBinding {
                image,
                rootfs: rootfs.to_path_buf(),
            },
        ])
        .expect("real rootfs binding verifies"),
    )
}

/// The env var naming a pre-provisioned `leases_dir` this drill may use for the STRICT
/// production [`UserNamespaceAllocator::try_new`] path. Sol's round-3 review: production strict
/// mode requires the leaf to ALREADY EXIST (owned by this process's euid, mode `0700` or
/// stricter) with an ancestor chain NOT writable by us — no ordinary test process can either
/// create that leaf itself (see `harden_and_verify_leases_dir`'s own doc) OR fabricate a
/// non-writable-by-us ancestor without real privilege, so this MUST come from an operator's
/// install step (e.g. `sudo install -d -m 0700 -o "$(whoami)" /opt/myelin-test/userns-leases`),
/// never something this drill provisions itself.
#[cfg(feature = "integration")]
pub(super) const USERNS_DRILL_LEASES_DIR_ENV: &str = "MYELIN_USERNS_DRILL_LEASES_DIR";

/// Sol's review (CT-007 slice 5b.2's live drill): this drill and the checkout-preparation live
/// drill (`checkout_preparation_5b2::checkout_preparation_runs_end_to_end_through_real_git_wire_and_runsc`)
/// share the SAME operator-provisioned `leases_dir` and may run concurrently under `cargo
/// test`'s default parallelism — the allocator's own directory lock is a PER-PROCESS lifetime
/// lock (see `UserNamespaceAllocator`'s own doc), not a per-call one, so two independent
/// `UserNamespaceAllocator::try_new` calls against the same directory in the SAME test binary
/// process would race nondeterministically. Both drills acquire this before touching
/// `leases_dir` at all, so only one is ever mid-flight.
#[cfg(feature = "integration")]
pub(super) static USERNS_DRILL_LEASES_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A [`RunscChild`] that records whether `kill()` was invoked, for
/// `discard_container_run_after_teardown_failure`'s tests below.
pub(super) struct CountingFakeRunsc {
    killed: Arc<AtomicBool>,
}

impl RunscChild for CountingFakeRunsc {
    fn kill(&mut self) -> Result<(), String> {
        self.killed.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn wait(&mut self) -> Result<i32, String> {
        Ok(0)
    }
}

pub(super) fn container_run_with_real_bundle_dir(
    killed: Arc<AtomicBool>,
) -> (ContainerRun, PathBuf) {
    let bundle_dir = std::env::temp_dir().join(format!(
        "myelin-gvisor-discard-bundle-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&bundle_dir).unwrap();
    let run = ContainerRun {
        child: Box::new(CountingFakeRunsc { killed }),
        bundle_dir: bundle_dir.clone(),
        result: SandboxResult::stub_ok(ResourceUsage {
            cpu_seconds: 1,
            mem_byte_seconds: 1,
        }),
        run_error: None,
    };
    (run, bundle_dir)
}

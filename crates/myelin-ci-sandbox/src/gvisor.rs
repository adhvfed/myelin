mod linux_capabilities;
pub use linux_capabilities::prepare_checkout_host_verification_capability;
use linux_capabilities::{
    capability_is_effective, capability_is_permitted, current_thread_capabilities,
    set_capability_effective, set_current_thread_capabilities, CAP_DAC_READ_SEARCH_NUMBER,
};

mod explicit_userns;
use explicit_userns::{
    apply_runsc_invocation_policy, delete_container, reject_security_capability_xattr,
    revalidated_explicit_userns_root_identity,
};
pub use explicit_userns::{
    preflight_explicit_userns_helpers, preflight_explicit_userns_policy,
    preflight_local_development_explicit_userns_policy, resolved_explicit_userns_helper_dir,
    resolved_explicit_userns_runsc_root, ENV_EXPLICIT_USERNS_HELPER_DIR,
    ENV_EXPLICIT_USERNS_RUNSC_ROOT,
};

mod cgroup;
pub use cgroup::{CgroupQuiescenceError, CgroupQuiescenceEvidence, MemoryCgroup};

mod preflight;
use preflight::runsc_bin;
pub use preflight::{
    preflight_gvisor_runner_host, probe_runsc_version, RunscProbeError, ENV_RUNSC_BIN,
};

mod oci_config;
pub(crate) use oci_config::OciWorkspaceMount;
#[cfg(test)]
use oci_config::CARGO_VENDOR_SOURCE_NAME;
use oci_config::{
    selected_cargo_vendor, validated_cargo_vendor_reference, FdBoundCargoVendor,
    OciExecutionLayout, WorkspaceProcessIdentity,
};
pub use oci_config::{
    OciConfig, CARGO_SOURCE_REPLACE_CONFIG, CARGO_SOURCE_REPLACE_ENV,
    CARGO_VENDOR_DIRECTORY_CONFIG, CARGO_VENDOR_DIRECTORY_ENV, ENV_CARGO_VENDOR_ASSET,
    OCI_CARGO_VENDOR_MOUNT, SERVER_CARGO_CONFIG_TOML, STRUCTURED_CARGO_HOME,
};

mod rootfs;
pub use rootfs::{
    build_gvisor_corpus_script, gvisor_drill_config_json, resolved_gvisor_git_rootfs,
    resolved_gvisor_rootfs, resolved_gvisor_rust_rootfs, verified_gvisor_git_rootfs,
    ENV_GVISOR_GIT_ROOTFS, ENV_GVISOR_ROOTFS, ENV_GVISOR_RUST_ROOTFS, GVISOR_CORPUS_SCRIPT,
    GVISOR_GIT_ROOTFS_SHA256, LINUX_RUST_V1_ROOTFS_SHA256, LINUX_SMALL_V1_ROOTFS_SHA256,
};

mod run;
use run::{
    require_oci_layout_matches_prepared_mode, run_production_container,
    run_production_container_streaming, stage_production_bundle, unique_suffix,
    PreparedRuntimeMode,
};

mod output_capture;
pub(crate) use output_capture::RunFailure;
use output_capture::{
    build_result, cap_total_job_output, run_and_capture, RunCaptureOptions, RunscOutcome,
    SpawnedRunsc, StdinSource, StdoutMode, StreamingOutput, NEVER_CANCELLED,
};

mod teardown;
#[cfg(test)]
use teardown::RuntimeTeardownIssue;
use teardown::{
    augment_run_failure_message, augment_run_failure_with_teardown,
    augment_settled_result_with_enabled_cleanup_failure, discard_container_run,
    discard_container_run_after_teardown_failure, finalize_and_merge, finalize_runtime,
    read_proc_cpu_seconds, settle_finalization, FinalizedRun, RuntimeFinalization,
    RuntimeTeardownError, RUNTIME_QUIESCE_TIMEOUT,
};
pub(crate) use teardown::{RuntimeNamespaceQuiescence, RuntimeQuiescenceEvidence};

mod workspace_lease;
#[cfg(test)]
use workspace_lease::settle_enabled_workspace_and_lease;
pub(crate) use workspace_lease::AcquisitionFailure;
use workspace_lease::{
    acquire_enabled_workspace, bind_prepared_lease_given, bind_then_continue,
    classify_workspace_deletion, cleanup_pre_bind_failure, join_diagnostics,
    settle_enabled_finalization, EnabledLaunchContext, EnabledWorkspaceRequest, LeaseBindState,
    RuntimeBinding, RuntimePreparation, WorkspaceDeletionOutcome, WorkspaceIntegration,
};

mod backend;
use backend::RunscProc;
pub use backend::{
    ContainerRun, GvisorBackend, GvisorBackendInitError, GvisorCheckoutConfig,
    GvisorCheckoutConfigError, GvisorError, GvisorWorkspaceConfig, RunscChild,
};

mod checkout_launch;

mod checkout_runtime;
#[cfg(any(test, feature = "test-support"))]
use checkout_runtime::AcquiredCheckoutRuntime;

mod workload_spec;
use workload_spec::{BoundWorkloadRefusal, WorkloadRotatedSpec};

mod git_wire_confinement;
pub use git_wire_confinement::{
    assert_repo_under_root, resolve_bare_repo_path, validate_wire_repo_slug, validate_wire_segment,
    WireError,
};

mod git_wire_codec;

mod git_wire_run;
use git_wire_run::{
    build_git_wire_job, build_git_wire_oci_config, run_git_wire_container_raw,
    stage_config_only_bundle, BundleCleanupProof, GitWireHopFinalization,
};
pub use git_wire_run::{
    GitWireSpec, RECEIVE_PACK_INGEST_SCRIPT, WIRE_QUARANTINE_MOUNT, WIRE_REPO_MOUNT,
    WIRE_STDIN_BOUND, WIRE_STDOUT_BOUND,
};

mod checkout_transport;
#[cfg(test)]
use checkout_transport::tempfile_for_checkout_pack;
use checkout_transport::{fetch_checkout_pack_within_parent_attempt_v2_given, GitWireHopExecutor};
pub(crate) use checkout_transport::{CheckoutTransportError, PrefetchedCheckoutPack};

mod checkout_preparation;
use checkout_preparation::{
    checkout_cleanup_plan, execute_cleanup_plan, resolve_checkout_preparation_permit,
    run_checkout_preparation_inner, RealCheckoutCleanupExecutor,
};
pub(crate) use checkout_preparation::{
    CheckoutPreparationError, CheckoutPreparationSpec, PreparedCheckoutEvidence,
    RetainedWorkloadOutcome,
};

#[cfg(feature = "test-support")]
pub mod runsc_driver;

#[cfg(test)]
mod test_fixtures;

#[cfg(any(test, feature = "test-support"))]
mod deterministic_substrate;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use deterministic_substrate::*;

#[cfg(any(test, feature = "test-support"))]
#[allow(private_interfaces)]
pub mod checkout_transport_test_support;

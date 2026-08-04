//! # The gVisor (`runsc`) second `SandboxBackend` (CI-P2 → P-237, M2; satisfies the CI-P28 floor early)
//!
//! **Owning architecture (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §5.1 ("gVisor is the named second backend behind the same `SandboxBackend` trait") + §5.3 (the
//! backend-independent hardening profile applied identically) + `sketches/01-isolation-model.md`
//! (Candidate A — gVisor / the `runsc` OCI runtime). **Contract:** `contract-index.md` row 8.4.
//!
//! ## Reconcile: the CI-P28 "gVisor second backend" — built early (P-237), PROMOTED at CI-P28 (P-423)
//! The original plan deferred gVisor to **CI-P28** (density/latency-economics-triggered). The
//! CI-P2 handoff INVERTED that: the host has `runsc` installed, so the backend SHAPE shipped early
//! (P-237) as the **named-second backend behind the SAME trait**. **CI-P28 (P-423) PROMOTES it**:
//! the escape drill now RE-RUNS the full adversarial corpus inside a real `runsc` (gVisor) sandbox
//! via a hardened OCI bundle (see [`build_gvisor_corpus_script`] / [`gvisor_drill_config_json`] +
//! `tests/escape_drill_gvisor_test.rs`), emitting a dated green attestation with gVisor EXERCISED —
//! the contract-8.4 permanent gate re-greened on the second backend.
//!
//! DEVIATION (documented, EI-01 §1): the FORMAL density/latency-economics trigger (measured at
//! CI-P30 / P-490) is downstream of P-423 and has NOT fired. The promotion is justified instead by
//! the binding DEV-REAL policy — this host has KVM + gVisor, so the sandbox-escape gate is a REAL
//! drill on both backends, not a floor. Proving the gate on the second backend NOW (rather than
//! waiting for the economics trigger) is strictly safer: it is a real green attestation, never a
//! weakened threshold.
//!
//! This is reconciliation, not a fork: the SAME [`SandboxBackend`](crate::SandboxBackend) trait, the
//! SAME mandatory [`HardeningProfile`](crate::hardening::HardeningProfile), the SAME four-guarantee
//! [`RunnerHooks`](crate::RunnerHooks) order, and the SAME host-side parser + attestation format.
//! gVisor uses the OCI/`runsc` path; Firecracker uses the microVM path. The drill governs which is
//! the production default (microVM, §5.1).
//!
//! ## `no-host-exec` (contract 1.6 / X-6 / AG-2)
//! Like the Firecracker backend, the REAL `runsc`-spawn site IS the sandbox seam's enforcement
//! mechanism (it *creates* the userspace-kernel boundary), not a bypass of it — a NAMED, LOUD
//! exclusion of this one file (registered in `lint-gate` + `tests/workspace_clean.rs`).
//!
//! ## Module map
//! This file is the module root: `mod` declarations and the re-exports that keep every item at the
//! `gvisor::…` path its callers already name. The backend itself reads roughly in launch order —
//! [`preflight`] resolves and vets the runtime, [`oci_config`] + [`rootfs`] build what `runsc` is
//! handed, [`run`]/[`output_capture`]/[`teardown`] execute and settle one container,
//! [`workspace_lease`] + [`backend`] + [`checkout_launch`] own the job-level orchestration, and
//! [`git_wire_run`] / [`checkout_transport`] / [`checkout_preparation`] are the git-shaped paths
//! layered on the same floor.

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
    resolved_explicit_userns_helper_dir, resolved_explicit_userns_runsc_root,
    ENV_EXPLICIT_USERNS_HELPER_DIR, ENV_EXPLICIT_USERNS_RUNSC_ROOT,
};

// ---------------------------------------------------------------------------------------------
// CT-003b (SI-017) — the out-of-band memory-cgroup enforcer for the gVisor workload lives in the
// dedicated `cgroup` submodule (see `gvisor/cgroup.rs`). Re-exported so the rest of `gvisor`, its
// callers, and the crate root keep naming these at the SAME paths (`gvisor::MemoryCgroup`, etc.).
// ---------------------------------------------------------------------------------------------
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
    selected_cargo_vendor, validated_cargo_vendor_reference, FdBoundCargoVendor, OciExecutionLayout,
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
    settle_enabled_finalization, EnabledLaunchContext, LeaseBindState, RuntimeBinding,
    RuntimePreparation, WorkspaceDeletionOutcome, WorkspaceIntegration,
};

mod backend;
use backend::RunscProc;
pub use backend::{
    ContainerRun, GvisorBackend, GvisorBackendInitError, GvisorCheckoutConfig,
    GvisorCheckoutConfigError, GvisorError, GvisorWorkspaceConfig, RunscChild,
};

mod checkout_launch;

// CT-007 slice 5b.3-6a (Sol's r4): the capsule types + their five approved accessors live in the
// DEDICATED private submodule `checkout_runtime`, where the struct fields are MODULE-PRIVATE — Rust's
// own module privacy, NOT a syntactic guard, forbids any other code (a sibling module, a free
// function, a macro expansion, OR a descendant module — there are none inside it) from NAMING
// `workload_cfg`/`enabled_context`/`session`/`acquired`/`prepared_checkout_evidence`. The reshaped Hop
// B entry lives inside the module and hands `run_checkout_preparation_inner` only &mut/& borrows
// obtained INSIDE the module. Re-exported so the rest of `gvisor` and its tests can name the types
// and the dormant Hop B entry without being able to reach their fields.
mod checkout_runtime;
// Dormant (5b.3-6a): no production caller yet — the re-export exists so 5b.3-6b/6c and the tests can
// name the types/entry. `#[allow(unused_imports)]` mirrors the capsule's own `#[allow(dead_code)]`.
#[allow(unused_imports)]
pub(crate) use checkout_runtime::{
    run_checkout_preparation_v2, AcquiredCheckoutRuntime, PreparedCheckoutRuntime,
};

// CT-007 slice 5b.3-6c (Sol's r5 finding 2): the workload-rotated spec wrapper lives in its own module
// so the inner `JobSpec` never escapes to be cloned/substituted (module-privacy fence). Re-exported so
// `checkout_runtime` can name the sealed wrapper + its refusal type without reaching the inner spec.
mod workload_spec;
#[allow(unused_imports)]
pub(crate) use workload_spec::{BoundWorkloadRefusal, WorkloadRotatedSpec};

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

// CT-007 slice 5b.3-6e.1 (DORMANT): the hardware-independent runsc-driver test seam. A SEPARATE
// module, gated `#[cfg(feature = "test-support")]`, so it is ABSENT from the ordinary dependency
// graph AND from the production-source dormancy pins (which never read this file). It substitutes
// ONLY the runtime executions (the workload `runsc` spawn) while driving the REAL preflight, REAL
// parent-attempt reservation (real hooks/authorities), REAL shared compute body, and REAL
// `SandboxCycleOutcome` routing — no Btrfs / /etc/subuid / KVM / runsc.
// CT-007 5b.3-6e.2 Stage A: `pub` (still `test-support`-gated) so the cross-crate §4 active-path tests
// can name the dormant Hop-B-injection selector by path. Every item stays gated + pinned
// production-zero; production never selects this module.
#[cfg(feature = "test-support")]
pub mod runsc_driver;

// ══════ the test and test-support modules ══════
//
// Declared LAST, below this banner: `source_pins::production_source()` stops here, which is the same
// exclusion these declarations used to get from being written after this file's own `mod tests`.
// Nothing below is reachable from any production composition root.

#[cfg(test)]
mod test_fixtures;

#[cfg(test)]
mod source_pins;

#[cfg(any(test, feature = "test-support"))]
mod deterministic_substrate;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use deterministic_substrate::*;

// ═════════════════ CT-007 slice 5b.3-6e.2 Stage A: the git-wire test-support substrate ═══════════
//
// The MINIMAL relocation (Sol's ruling: NOT the whole fixture cluster) of the git-wire fakes the
// `#[cfg(test)]` `checkout_transport_5b3_3` module grew, lifted to
// `#[cfg(any(test, feature = "test-support"))]` so the hardware-independent runsc-driver seam
// (`gvisor/runsc_driver.rs`, `#[cfg(feature = "test-support")]`) and the cross-crate §4 active-path
// tests can drive the REAL checkout orchestrator with a scripted two-call Hop-A executor. Every item
// is gated, ABSENT from ordinary builds, and reachable from NO production composition root. The
// `#[cfg(test)]` callers (`checkout_preparation_5b2`, `checkout_transport_5b3_3`, `test_fixtures`
// for `FakeRunsc`) re-import these by path so their existing tests keep compiling.
#[cfg(any(test, feature = "test-support"))]
#[allow(dead_code)]
// several helpers are consumed only by `#[cfg(test)]` callers and/or the driver.
// The executor helpers name the crate-private `RuntimeFinalization`/`GitWireHopFinalization` in their
// `pub(crate)` signatures — an intentional crate-internal seam, never a public leak.
#[allow(private_interfaces)]
// CT-007 slice 5b.3-6e.2 Stage A: `pub` (still `#[cfg(any(test, feature = "test-support"))]`-gated, so
// ABSENT from ordinary/production builds) ONLY so the cross-crate §4 active-path tests can name the
// three `pub` helpers below by path. Every OTHER item stays `pub(crate)` — an intentional crate-internal
// seam, never a public leak — and the whole module remains excluded from `production_source()` and
// pinned production-zero (see `ordinary_build_and_production_root_pins`).
pub mod checkout_transport_test_support;

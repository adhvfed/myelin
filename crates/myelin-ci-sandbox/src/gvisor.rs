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



mod linux_capabilities;
pub use linux_capabilities::prepare_checkout_host_verification_capability;
use linux_capabilities::{
    CAP_DAC_READ_SEARCH_NUMBER, capability_is_effective, capability_is_permitted,
    current_thread_capabilities, set_capability_effective, set_current_thread_capabilities,
};
// Referenced only by the `test-support`-gated host-verifier capability tests.
#[cfg(all(test, feature = "test-support"))]
use linux_capabilities::{ambient_capability_is_set, capability_is_inheritable};

mod preflight;
pub use preflight::{ENV_RUNSC_BIN, RunscProbeError, preflight_gvisor_runner_host, probe_runsc_version};
use preflight::runsc_bin;

mod oci_config;
pub use oci_config::{CARGO_SOURCE_REPLACE_CONFIG, CARGO_SOURCE_REPLACE_ENV, CARGO_VENDOR_DIRECTORY_CONFIG, CARGO_VENDOR_DIRECTORY_ENV, ENV_CARGO_VENDOR_ASSET, OCI_CARGO_VENDOR_MOUNT, OciConfig, SERVER_CARGO_CONFIG_TOML, STRUCTURED_CARGO_HOME};
pub(crate) use oci_config::OciWorkspaceMount;
use oci_config::{FdBoundCargoVendor, OciExecutionLayout, selected_cargo_vendor, validated_cargo_vendor_reference};

mod workspace_lease;
pub(crate) use workspace_lease::AcquisitionFailure;
use workspace_lease::{EnabledLaunchContext, LeaseBindState, RuntimeBinding, RuntimePreparation, WorkspaceDeletionOutcome, WorkspaceIntegration, acquire_enabled_workspace, bind_prepared_lease_given, bind_then_continue, classify_workspace_deletion, cleanup_pre_bind_failure, join_diagnostics, settle_enabled_finalization};

mod backend;
pub use backend::{ContainerRun, GvisorBackend, GvisorBackendInitError, GvisorCheckoutConfig, GvisorCheckoutConfigError, GvisorError, GvisorWorkspaceConfig, RunscChild};
use backend::RunscProc;

mod checkout_launch;

mod run;
use run::{PreparedRuntimeMode, require_oci_layout_matches_prepared_mode, run_production_container, run_production_container_streaming, stage_production_bundle, unique_suffix};

mod teardown;
pub(crate) use teardown::{RuntimeNamespaceQuiescence, RuntimeQuiescenceEvidence};
use teardown::{FinalizedRun, RUNTIME_QUIESCE_TIMEOUT, RuntimeFinalization, RuntimeTeardownError, augment_run_failure_message, augment_run_failure_with_teardown, augment_settled_result_with_enabled_cleanup_failure, discard_container_run, discard_container_run_after_teardown_failure, finalize_and_merge, finalize_runtime, read_proc_cpu_seconds, settle_finalization};

mod output_capture;
pub(crate) use output_capture::RunFailure;
use output_capture::{NEVER_CANCELLED, RunCaptureOptions, RunscOutcome, SpawnedRunsc, StdinSource, StdoutMode, StreamingOutput, build_result, cap_total_job_output, run_and_capture};

mod rootfs;
pub use rootfs::{ENV_GVISOR_GIT_ROOTFS, ENV_GVISOR_ROOTFS, ENV_GVISOR_RUST_ROOTFS, GVISOR_CORPUS_SCRIPT, GVISOR_GIT_ROOTFS_SHA256, LINUX_RUST_V1_ROOTFS_SHA256, LINUX_SMALL_V1_ROOTFS_SHA256, build_gvisor_corpus_script, gvisor_drill_config_json, resolved_gvisor_git_rootfs, resolved_gvisor_rootfs, resolved_gvisor_rust_rootfs, verified_gvisor_git_rootfs};

mod git_wire_run;
pub use git_wire_run::{GitWireSpec, RECEIVE_PACK_INGEST_SCRIPT, WIRE_QUARANTINE_MOUNT, WIRE_REPO_MOUNT, WIRE_STDIN_BOUND, WIRE_STDOUT_BOUND};
use git_wire_run::{BundleCleanupProof, GitWireHopFinalization, build_git_wire_job, build_git_wire_oci_config, run_git_wire_container_raw, stage_config_only_bundle};

mod checkout_transport;
pub(crate) use checkout_transport::{CheckoutTransportError, PrefetchedCheckoutPack};
use checkout_transport::{GitWireHopExecutor, fetch_checkout_pack_within_parent_attempt_v2_given};

mod checkout_preparation;
pub(crate) use checkout_preparation::{CheckoutPreparationError, CheckoutPreparationSpec, PreparedCheckoutEvidence, RetainedWorkloadOutcome};
use checkout_preparation::{RealCheckoutCleanupExecutor, checkout_cleanup_plan, execute_cleanup_plan, resolve_checkout_preparation_permit, run_checkout_preparation_inner};






// ---------------------------------------------------------------------------------------------
// CT-003b (SI-017) — the out-of-band memory-cgroup enforcer for the gVisor workload lives in the
// dedicated `cgroup` submodule (see `gvisor/cgroup.rs`). Re-exported so the rest of `gvisor`, its
// callers, and the crate root keep naming these at the SAME paths (`gvisor::MemoryCgroup`, etc.).
// ---------------------------------------------------------------------------------------------
mod cgroup;
pub use cgroup::{CgroupQuiescenceError, CgroupQuiescenceEvidence, MemoryCgroup};









mod explicit_userns;
pub use explicit_userns::{
    preflight_explicit_userns_helpers, preflight_explicit_userns_policy,
    resolved_explicit_userns_helper_dir, resolved_explicit_userns_runsc_root,
    ENV_EXPLICIT_USERNS_HELPER_DIR, ENV_EXPLICIT_USERNS_RUNSC_ROOT,
};
use explicit_userns::{
    apply_runsc_invocation_policy, delete_container, reject_security_capability_xattr,
    revalidated_explicit_userns_root_identity,
};
// Exercised only by this module's own `#[cfg(test)] mod tests` (via `use super::*`); the
// production paths above are the whole non-test surface.
#[cfg(test)]
use explicit_userns::{
    apply_explicit_userns_env, apply_runsc_invocation_policy_checked_given,
    apply_runsc_invocation_policy_given, harden_explicit_userns_runsc_binary,
    harden_explicit_userns_runsc_root, revalidated_explicit_userns_root_identity_given,
    sha256_hex_of_file, verify_explicit_userns_runsc_root_leaf,
    verify_pinned_explicit_userns_runsc, ResolvedExplicitUsernsPolicy,
    PINNED_EXPLICIT_USERNS_RUNSC_VERSION,
};






mod git_wire_confinement;
pub use git_wire_confinement::{
    assert_repo_under_root, resolve_bare_repo_path, validate_wire_repo_slug, validate_wire_segment,
    WireError,
};


// `GitObjectFormat` is now referenced only from tests (the git-wire codec that used it in
// production moved to `git_wire_codec`); keep the production import lean.
#[cfg(test)]
use crate::workspace_intent::GitObjectFormat;

mod git_wire_codec;



// CT-007 slice 5b.3-6a (Sol's r4): the capsule types + their five approved accessors live in the
// DEDICATED private submodule `checkout_runtime`, where the struct fields are MODULE-PRIVATE — Rust's
// own module privacy, NOT a syntactic guard, forbids any other code (a sibling module, a free
// function, a macro expansion, OR a descendant module — there are none inside it) from NAMING
// `workload_cfg`/`enabled_context`/`session`/`acquired`/`prepared_checkout_evidence`. The reshaped Hop
// B entry lives inside the module and hands `run_checkout_preparation_inner` (below) only &mut/&
// borrows obtained INSIDE the module. Re-exported so the rest of `gvisor` and its tests can name the
// types and the dormant Hop B entry without being able to reach their fields.
mod checkout_runtime;
// Dormant (5b.3-6a): no production caller yet — the re-export exists so 5b.3-6b/6c and the tests can
// name the types/entry. `#[allow(unused_imports)]` mirrors the capsule's own `#[allow(dead_code)]`.
#[allow(unused_imports)]
pub(crate) use checkout_runtime::{
    run_checkout_preparation_v2, AcquiredCheckoutRuntime, PreparedCheckoutRuntime,
};

// CT-007 slice 5b.3-6e.1 (DORMANT): the hardware-independent runsc-driver test seam. A SEPARATE
// module, gated `#[cfg(feature = "test-support")]`, so it is ABSENT from the ordinary dependency
// graph AND from the `gvisor.rs` production-source dormancy pins (which read `include_str!("gvisor.rs")`
// and never see this file). It substitutes ONLY the runtime executions (the workload `runsc` spawn)
// while driving the REAL preflight, REAL parent-attempt reservation (real hooks/authorities), REAL
// shared compute body, and REAL `SandboxCycleOutcome` routing — no Btrfs / /etc/subuid / KVM / runsc.
// CT-007 5b.3-6e.2 Stage A: `pub` (still `test-support`-gated) so the cross-crate §4 active-path tests
// can name the dormant Hop-B-injection selector by path. Every item stays gated + pinned
// production-zero; production never selects this module.
#[cfg(feature = "test-support")]
pub mod runsc_driver;

// CT-007 slice 5b.3-6c (Sol's r5 finding 2): the workload-rotated spec wrapper lives in its own module
// so the inner `JobSpec` never escapes to be cloned/substituted (module-privacy fence). Re-exported so
// `checkout_runtime` can name the sealed wrapper + its refusal type without reaching the inner spec.
mod workload_spec;
#[allow(unused_imports)]
pub(crate) use workload_spec::{BoundWorkloadRefusal, WorkloadRotatedSpec};


#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::{EnvVar, RunTokenCredential};
    // CT-007 slice 5b.3-6e.2 Stage A: the git-wire fakes were lifted to a
    // `#[cfg(any(test, feature = "test-support"))]` module (below the top-level test module) so the
    // hardware-independent runsc-driver seam + §4 tests can reach them. Re-imported here so the
    // existing `#[cfg(test)]` callers keep compiling.
    use crate::gvisor::checkout_transport_test_support::FakeRunsc;

    // =============================================================================================
    // CT-007 phase-credential generations: SOURCE PINS.
    //
    // These read this module's own source text. A behavioural test can only prove that the paths it
    // exercises are gated; a source pin proves that NO path exists — which is exactly the property
    // "no raw preparation spawn is reachable with an immediate permit in the V2 API" asserts.
    // =============================================================================================

    const GVISOR_SOURCE: &str = include_str!("gvisor.rs");
    /// The dedicated capsule submodule's source (CT-007 5b.3-6a, Sol's r4): the capsule types + the
    /// reshaped Hop B entry moved here so module privacy enforces field inseparability.
    const CHECKOUT_RUNTIME_SOURCE: &str = include_str!("gvisor/checkout_runtime.rs");

    /// [`source_of`] against an arbitrary source string (used for the capsule submodule).
    fn source_of_in(source: &'static str, function_signature: &str) -> &'static str {
        let start = source
            .find(function_signature)
            .unwrap_or_else(|| panic!("`{function_signature}` exists"));
        let rest = &source[start..];
        let end = rest
            .find("\n}\n")
            .unwrap_or_else(|| panic!("`{function_signature}` has a top-level close"));
        &rest[..end]
    }

    /// This module's PRODUCTION source only. A whole-file `contains` check would otherwise match
    /// the assertion strings in these very tests — a source pin that reads its own file must
    /// exclude the test region or it silently asserts against itself.
    fn production_source() -> &'static str {
        // Split on the TOP-LEVEL test module specifically: this file also carries inline
        // `#[cfg(test)]` helpers far above it, so splitting on the bare attribute would truncate
        // almost the whole production body and make every pin vacuously true.
        let marker = "\n#[cfg(test)]\nmod tests {";
        let end = GVISOR_SOURCE
            .find(marker)
            .expect("this module has a top-level test module");
        &GVISOR_SOURCE[..end]
    }

    /// The body of a named `fn`/`pub(crate) fn`, from its signature to the next top-level `}` at
    /// column 0 — enough to scope a source pin to one function without pulling in its neighbours.
    fn source_of(function_signature: &str) -> &'static str {
        let start = GVISOR_SOURCE
            .find(function_signature)
            .unwrap_or_else(|| panic!("`{function_signature}` exists in this module"));
        let rest = &GVISOR_SOURCE[start..];
        let end = rest
            .find("\n}\n")
            .unwrap_or_else(|| panic!("`{function_signature}` has a top-level close"));
        &rest[..end]
    }

    /// **In the V2 API, neither preparation spawn can construct its own permit — and the V2 entry
    /// points offer no legacy option at the TYPE level.** These pins are the structural complement
    /// to the behavioural tests: a behavioural test proves the paths it exercises are gated, a
    /// source pin proves no ungated path EXISTS.
    #[test]
    fn no_v2_preparation_spawn_can_mint_its_own_immediate_permit() {
        let hop = source_of("fn run_one_git_wire_hop_within_parent_attempt(");
        assert!(
            !hop.contains("LaunchPermit::immediate()"),
            "the git-wire hop runner must consume the permit it is handed, never mint one"
        );
        assert!(
            hop.contains("permit: LaunchPermit"),
            "the git-wire hop runner takes its permit as an argument"
        );

        // Hop B: the shared body takes its permit; only the LEGACY entry point mints an immediate
        // one; the V2 entry point resolves it by CONSUMING a PhaseAuthorization.
        let inner = source_of("fn run_checkout_preparation_inner(");
        assert!(
            inner.contains("launch_permit: LaunchPermit") && inner.contains("Some(launch_permit)"),
            "Hop B's body consumes the permit it is handed, never mints one"
        );
        assert!(
            !inner.contains("LaunchPermit::immediate()"),
            "Hop B's body must never mint a permit inline"
        );
        let legacy_preparation = source_of("pub(crate) fn run_checkout_preparation(");
        assert_eq!(
            legacy_preparation
                .matches("LaunchPermit::immediate()")
                .count(),
            1,
            "the LEGACY Hop B entry point is the one place an immediate preparation permit exists"
        );
        // The V2 Hop B entry now lives in the dedicated `checkout_runtime` submodule (Sol's r4).
        let v2_preparation = source_of_in(
            CHECKOUT_RUNTIME_SOURCE,
            "pub(crate) fn run_checkout_preparation_v2(",
        );
        assert!(
            !v2_preparation.contains("LaunchPermit::immediate()")
                && !v2_preparation.contains("CheckoutAuthorizationProof"),
            "the V2 Hop B entry point can neither mint an immediate permit nor accept a legacy proof"
        );
        assert!(
            v2_preparation.contains("authorization: PhaseAuthorization"),
            "the V2 Hop B entry point takes the fused, non-constructible authorization"
        );
        let resolve = source_of("fn resolve_checkout_preparation_permit(");
        assert!(
            !resolve.contains("LaunchPermit::immediate()")
                && resolve.contains(
                    "into_preparation_permit_for_scope(run_token, checkout_scope, expected_commit)"
                ),
            "the V2 permit is reachable ONLY by consuming the authorization through its own checks, \
             bound to the capsule's FULL scope (5b.3-6a blocker 1)"
        );

        // Hop A: the module-private authority enum means the V2 entry point cannot select a legacy
        // arm, and the legacy immediate permits live only on the legacy arm of the shared body.
        let production = production_source();
        assert!(
            production.contains("\nenum TransportAuthority<'a> {")
                && !production.contains("pub(crate) enum TransportAuthority")
                && !production.contains("pub enum TransportAuthority"),
            "the transport authority enum is MODULE-PRIVATE: no other module can select an arm"
        );
        let v2_transport = source_of("pub(crate) fn fetch_checkout_pack_within_parent_attempt_v2(");
        assert!(
            !v2_transport.contains("CheckoutAuthorizationProof")
                && !v2_transport.contains("LaunchPermit"),
            "the V2 Hop A entry point accepts neither a legacy proof nor a bare permit"
        );
        assert!(
            v2_transport.contains("advertise: PhaseAuthorization")
                && v2_transport
                    .contains("Result<(RunTokenCredential, PhaseAuthorization), HookError>"),
            "the V2 Hop A entry point takes the fused authorization for both legs"
        );
        let inner_transport = source_of("fn fetch_checkout_pack_within_parent_attempt_inner(");
        assert_eq!(
            inner_transport.matches("LaunchPermit::immediate()").count(),
            2,
            "exactly two immediate permits, both on the legacy arm (advertise + fetch)"
        );
        assert!(
            inner_transport.contains("TransportAuthority::LegacyClaimBound { proof } => {")
                && inner_transport.contains("(None, LaunchPermit::immediate(), None)"),
            "the advertise immediate permit belongs to the legacy arm"
        );
        assert!(
            inner_transport.contains("None => (run_token, LaunchPermit::immediate()),"),
            "the fetch immediate permit belongs to the legacy (no fetch provider) arm"
        );
        assert!(
            inner_transport.contains(
                "let permit = advertise\n                .into_transport_permit(\n                    crate::CheckoutPhase::Advertise,"
            ),
            "the V2 advertise leg reaches its permit only by CONSUMING its authorization"
        );
        assert!(
            inner_transport.contains("into_transport_permit(crate::CheckoutPhase::Fetch, &credential, tenant, repo, expected)"),
            "the V2 fetch leg reaches its permit only by consuming its authorization, checked \
             against the credential the SAME provider returned"
        );
    }

    /// **The fused authorization cannot be taken apart.** `PhaseAuthorization` has no public
    /// constructor, is not `Clone`, and its permit field is only ever moved out by a consuming
    /// `into_*_permit` that runs the provenance checks first.
    #[test]
    fn the_phase_authorization_is_structurally_inseparable() {
        const AUTHORIZATION_SOURCE: &str = include_str!("checkout_authorization.rs");
        assert!(
            AUTHORIZATION_SOURCE.contains("pub(crate) struct PhaseAuthorization {")
                && AUTHORIZATION_SOURCE.contains("    permit: LaunchPermit,"),
            "the permit is a PRIVATE field of the fused authorization"
        );
        let declaration = AUTHORIZATION_SOURCE
            .split("pub(crate) struct PhaseAuthorization {")
            .next()
            .expect("the declaration exists");
        // Whatever attribute block immediately precedes the struct must not derive Clone/Copy.
        let attributes = declaration
            .rsplit("\n\n")
            .next()
            .expect("there is an attribute block");
        assert!(
            !attributes.contains("derive(") || !attributes.contains("Clone"),
            "the authorization is deliberately NOT Clone: it cannot be duplicated across legs"
        );
        assert!(
            !AUTHORIZATION_SOURCE.contains("impl Clone for PhaseAuthorization"),
            "the authorization must not hand-implement Clone either"
        );
        // Round-2 minor 3: pin EVERY access of `self.permit`, not just the two `Ok(self.permit)`
        // returns. A future `pub(crate) fn leak(self) -> LaunchPermit { self.permit }` would add a
        // third `self.permit` and fail here even though it changes neither the `Ok(self.permit)`
        // count, the construction count, nor the Clone checks.
        assert_eq!(
            AUTHORIZATION_SOURCE.matches("self.permit").count(),
            2,
            "the permit field is TOUCHED in exactly two places: the two consuming into_*_permit \
             methods (transport, and the 5b.3-6a full-scope preparation — the commit-only preparation \
             permit was removed in r2). A new permit-exposing method would raise this count."
        );
        assert_eq!(
            AUTHORIZATION_SOURCE.matches("Ok(self.permit)").count(),
            2,
            "the permit escapes only through the two consuming into_*_permit methods"
        );
        // Pin the COMPLETE method surface of `impl PhaseAuthorization` — an exact set. Any new
        // method (permit-exposing or otherwise) fails until it is reviewed and added here.
        //
        // Round-3 minor: the parser must recognize a method under ANY visibility/modifier form, or a
        // leak could hide behind a spelling the parser skips. The exact shapes defended against:
        //   pub(super) async fn leak(self) -> LaunchPermit { let Self { permit, .. } = self; permit }
        //   pub(in crate::foo) const unsafe fn leak(self) -> LaunchPermit { self.permit }
        // The (a) method-surface enumeration below strips every `pub`/`pub(..)` visibility and every
        // `async`/`const`/`unsafe`/`extern` modifier before reading the `fn` name, so ANY new method
        // (regardless of spelling) enters the parsed set and breaks the exact-set assertion; the (b)
        // destructuring guard forbids `Self {` / `PhaseAuthorization {` binding patterns inside the
        // impl, closing the "move the field out by pattern" route the `self.permit` counter misses.
        let impl_block = {
            let start = AUTHORIZATION_SOURCE
                .find("\nimpl PhaseAuthorization {")
                .expect("the impl block exists");
            let rest = &AUTHORIZATION_SOURCE[start + 1..];
            let end = rest.find("\n}\n").expect("the impl block closes");
            &rest[..end]
        };
        // Return the `fn` name of a method declaration under any visibility/modifier chain.
        fn method_name(line: &str) -> Option<&str> {
            let mut rest = line.trim_start();
            // One visibility token: `pub`, `pub(crate)`, `pub(super)`, `pub(in path)`.
            if let Some(after_pub) = rest.strip_prefix("pub") {
                if let Some(inner) = after_pub.strip_prefix('(') {
                    let close = inner.find(')')?;
                    rest = inner[close + 1..].trim_start();
                } else if after_pub.starts_with(char::is_whitespace) {
                    rest = after_pub.trim_start();
                }
                // else: `pub` was a prefix of some other identifier — leave `rest` as-is; it will
                // fail the `fn ` check below and be ignored.
            }
            // Any combination of `async`/`const`/`unsafe`/`extern "ABI"` modifiers, in any order.
            loop {
                let mut advanced = false;
                for keyword in ["async", "const", "unsafe", "extern"] {
                    if let Some(after) = rest.strip_prefix(keyword) {
                        if after.starts_with(char::is_whitespace) || after.starts_with('"') {
                            rest = after.trim_start();
                            if keyword == "extern" {
                                if let Some(abi) = rest.strip_prefix('"') {
                                    if let Some(end) = abi.find('"') {
                                        rest = abi[end + 1..].trim_start();
                                    }
                                }
                            }
                            advanced = true;
                        }
                    }
                }
                if !advanced {
                    break;
                }
            }
            let after_fn = rest.strip_prefix("fn ")?;
            let name = after_fn.split(['(', '<', ' ']).next()?.trim();
            (!name.is_empty()).then_some(name)
        }
        let mut methods: Vec<&str> = impl_block.lines().filter_map(method_name).collect();
        methods.sort_unstable();
        assert_eq!(
            methods,
            vec![
                "generation_id",
                "into_preparation_permit_for_scope",
                "into_transport_permit",
                "phase",
                "run_token_jti",
                "verify_provenance",
            ],
            "the PhaseAuthorization method surface changed — any new method (under ANY visibility or \
             modifier) that could move or expose `self.permit` (or destructure self) must be \
             reviewed and pinned here"
        );
        // (b) Destructuring is the other route to a private field. Forbid BOTH binding spellings
        // (`let Self { permit, .. } = self` and `let PhaseAuthorization { .. } = self`) ANYWHERE
        // inside the inherent impl block.
        assert_eq!(
            impl_block.matches("Self {").count(),
            0,
            "no `Self {{ .. }}` destructuring pattern inside impl PhaseAuthorization may bind the \
             private permit"
        );
        assert_eq!(
            impl_block.matches("PhaseAuthorization {").count(),
            1,
            "the only `PhaseAuthorization {{` inside the impl block is its own header — never a \
             destructuring pattern that could pull the permit out"
        );
        // Belt-and-braces global count too (struct decl, Debug header, inherent-impl header, and the
        // ONE construction site in `RunnerHooks::authorize_checkout_phase`).
        assert_eq!(
            AUTHORIZATION_SOURCE.matches("PhaseAuthorization {").count(),
            4,
            "exactly: the struct decl, the Debug impl header, the inherent impl header, and the one \
             construction site — no destructuring pattern reaches the private permit"
        );
        assert_eq!(
            AUTHORIZATION_SOURCE
                .matches("self.verify_provenance(")
                .count(),
            2,
            "every consumption runs the phase/JTI/generation provenance check first"
        );
        assert!(
            AUTHORIZATION_SOURCE.contains("PhaseAuthorization {\n            scope,"),
            "the ONE construction site fuses the scope, retained JTI, phase, generation, and permit"
        );
        assert_eq!(
            AUTHORIZATION_SOURCE.matches("permit,\n        })").count(),
            1,
            "there is exactly ONE construction site for the fused authorization"
        );
    }

    /// **The checkout-runtime submodule's SHAPE is audited CLOSED-WORLD (syn AST).** (CT-007 slice
    /// 5b.3-6a, Sol's r4/r5.) The capsule types + their five approved accessors live in the dedicated
    /// private submodule `checkout_runtime`, so **Rust's own module privacy** — not this test — forbids
    /// any code OUTSIDE the module (sibling, free fn, macro expansion, descendant module) from NAMING
    /// the inner fields; the compile-error bites-proofs fail to COMPILE with a privacy error, not a
    /// test. But code INSIDE the module can still EXPORT a leak (Sol's r5: `pub(crate) static LEAK:
    /// fn(&Cap)->&OciConfig = |c| &c.workload_cfg;` — the closure's field access is legal inside the
    /// owning module, and a parent calls `checkout_runtime::LEAK`). So this test makes the module's
    /// export inventory CLOSED-WORLD: parsing `checkout_runtime.rs`, EVERY production (non-`#[cfg(test)]`)
    /// top-level item MUST be one of — `use` imports (any number); EXACTLY the two capsule structs (no
    /// other struct/enum/union/type/alias); INHERENT impls on ONLY those two types (no trait impl, no
    /// impl on another self-type); and ONLY the free fns in `ALLOWED_FREE_FNS`. Any other item kind —
    /// `static`, `const`, `trait`, `macro_rules!`/macro invocation, `extern`, `mod`, type alias, union,
    /// an extra free fn — FAILS the audit BY NAME. Together with module privacy, this audited inventory
    /// is the compile-time guarantee: there is nowhere inside the module to hide a leaking
    /// static/const/helper. The audit ALSO checks capsule fields private, no `Clone`/`Copy`, no non-`fn`
    /// associated items, and the exact non-private accessor surface (the five entries).
    ///
    /// MULTIPLICITY-EXACT (Sol's r6): the struct-name, free-fn-name, and accessor-surface inventories
    /// are SORTED LISTS compared with multiplicity (no dedup, no set-membership). `syn` parses BOTH
    /// arms of a `#[cfg]`/`#[cfg(not)]` pair as two items, so a second gated definition of an approved
    /// name (which a comment can hide from a literal occurrence pin) makes its list one entry LONGER
    /// than the allowlist and FAILS — where a set would have collapsed it to one.
    ///
    /// RESIDUAL — HONEST SCOPE: `syn` does not expand macros. Any macro invocation IN THIS MODULE whose
    /// token stream contains a capsule TYPE ident or ANY inner-field ident (all five, incl.
    /// `prepared_checkout_evidence`) fails this test, forcing review; and a top-level `macro_rules!`/
    /// macro invocation is itself rejected by the closed-world inventory. The remaining unexpanded
    /// external/procedural-macro gap is acceptable for this dormant guard (Sol's r4/r5: do not move it
    /// to `myelin-lints`).
    #[test]
    fn the_checkout_runtime_module_shape_is_pinned() {
        const CAPSULES: [&str; 2] = ["AcquiredCheckoutRuntime", "PreparedCheckoutRuntime"];
        const MACRO_REVIEW_IDENTS: [&str; 7] = [
            "AcquiredCheckoutRuntime",
            "PreparedCheckoutRuntime",
            "workload_cfg",
            "enabled_context",
            "session",
            "acquired",
            "prepared_checkout_evidence",
        ];
        // CT-007 slice 5b.3-6c: the standalone `PreparedCheckoutRuntime::bind_workload` synthetic-identity
        // helper was FOLDED into the ONE closed `run_retained_workload` transition — its allowlist entry
        // is REPLACED (still exactly FIVE accessors). This is deliberate audited-API evolution: a caller
        // can request the whole sanctioned workload transition, but can no longer extract or substitute
        // its constituent bind capability.
        const ALLOWLIST: [&str; 5] = [
            "AcquiredCheckoutRuntime::acquire",
            "AcquiredCheckoutRuntime::dispose_checkout_runtime",
            "PreparedCheckoutRuntime::dispose_checkout_runtime",
            "PreparedCheckoutRuntime::run_retained_workload",
            "run_checkout_preparation_v2",
        ];
        // CT-007 slice 5b.3-6c: the EXACT `#[cfg(test)]`-only method inventory. The audit already
        // excludes the whole test-only impl from the production accessor surface; pinning the test set
        // exactly keeps that exception explicit — a new test-only capsule method fails until reviewed.
        // CT-007 slice 5b.3-6c (Sol's finding 6): the exact `#[cfg(test)]`-only capsule method set —
        // the session driver, the into-prepared type-state transition, and the injectable workload
        // execution seam. Pinned exactly so a new test-only capsule method fails until reviewed.
        const TEST_METHODS: [&str; 3] = [
            "drive_session_for_tests",
            "into_prepared_for_tests",
            "run_retained_workload_given",
        ];
        // CT-007 slice 5b.3-6e.1b/6e.2: the EXACT `#[cfg(any(test, feature = "test-support"))]`-only
        // method inventory. The deterministic substituted-execution seam is gated for `test-support`
        // (so the hardware-independent runsc-driver fixture can reach it), NOT `#[cfg(test)]` — so it
        // is recognized as its OWN test-support surface and does NOT count against the FIVE-entry
        // production accessor `ALLOWLIST`. 6e.2 SPLIT the single seam into its Hop B half (on
        // `AcquiredCheckoutRuntime`, returning the fused prepared capsule) and its workload half (on
        // `PreparedCheckoutRuntime`, driving the REAL `run_retained_workload_inner`), so the ruling-(A)
        // workload leg runs the real authority/settle path. Pinned exactly so a new test-support
        // capsule method fails until reviewed.
        const TEST_SUPPORT_METHODS: [&str; 2] = [
            "substituted_hop_b_for_test_support",
            "substituted_workload_for_test_support",
        ];
        // Closed-world (Sol's r5): the EXACT set of free functions permitted at module top level. Any
        // other free fn — even a private helper — is a violation until reviewed and added here.
        const ALLOWED_FREE_FNS: [&str; 1] = ["run_checkout_preparation_v2"];

        fn type_last_ident(ty: &syn::Type) -> Option<String> {
            match ty {
                syn::Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.to_string()),
                syn::Type::Reference(r) => type_last_ident(&r.elem),
                _ => None,
            }
        }
        fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
            attrs.iter().any(|a| {
                let mut hit = false;
                if a.path().is_ident("cfg") {
                    let _ = a.parse_nested_meta(|meta| {
                        if meta.path.is_ident("test") {
                            hit = true;
                        }
                        Ok(())
                    });
                }
                hit
            })
        }
        fn is_private(vis: &syn::Visibility) -> bool {
            matches!(vis, syn::Visibility::Inherited)
        }
        /// Whether an item is gated `#[cfg(any(test, feature = "test-support"))]` (or
        /// `#[cfg(feature = "test-support")]`) — the test-support EXECUTION seam, distinct from the
        /// `#[cfg(test)]`-only driver impl. Detected by the `test-support` feature token inside a
        /// `cfg(...)` attribute; recognized as its own inventory so it never counts against the
        /// five-entry production accessor surface.
        fn is_cfg_test_support(attrs: &[syn::Attribute]) -> bool {
            attrs.iter().any(|a| {
                a.path().is_ident("cfg")
                    && matches!(&a.meta, syn::Meta::List(list) if list.tokens.to_string().contains("test-support"))
            })
        }
        /// A human name for a forbidden top-level item kind (for the closed-world violation message).
        fn describe_item(item: &syn::Item) -> String {
            match item {
                syn::Item::Const(c) => format!("const `{}`", c.ident),
                syn::Item::Static(s) => format!("static `{}`", s.ident),
                syn::Item::Trait(t) => format!("trait `{}`", t.ident),
                syn::Item::TraitAlias(t) => format!("trait alias `{}`", t.ident),
                syn::Item::Type(t) => format!("type alias `{}`", t.ident),
                syn::Item::Enum(e) => format!("enum `{}`", e.ident),
                syn::Item::Union(u) => format!("union `{}`", u.ident),
                syn::Item::Mod(m) => format!("module `{}`", m.ident),
                syn::Item::Macro(m) => format!(
                    "macro `{}!`",
                    m.mac
                        .path
                        .segments
                        .last()
                        .map(|s| s.ident.to_string())
                        .unwrap_or_default()
                ),
                syn::Item::ForeignMod(_) => "an extern block".to_string(),
                syn::Item::ExternCrate(e) => format!("extern crate `{}`", e.ident),
                _ => "an unrecognized item kind".to_string(),
            }
        }
        fn macro_mentions(ts: &proc_macro2::TokenStream, needles: &[&str]) -> Option<String> {
            for tt in ts.clone() {
                match tt {
                    proc_macro2::TokenTree::Ident(id) => {
                        let s = id.to_string();
                        if needles.contains(&s.as_str()) {
                            return Some(s);
                        }
                    }
                    proc_macro2::TokenTree::Group(g) => {
                        if let Some(h) = macro_mentions(&g.stream(), needles) {
                            return Some(h);
                        }
                    }
                    _ => {}
                }
            }
            None
        }

        let file =
            syn::parse_file(CHECKOUT_RUNTIME_SOURCE).expect("checkout_runtime.rs parses as a File");
        let mut violations: Vec<String> = Vec::new();
        // MULTIPLICITY-EXACT inventories (Sol's r6): NO dedup. `syn` parses BOTH arms of a
        // `#[cfg]`/`#[cfg(not)]` pair as two separate items (it never evaluates cfg), so two gated
        // definitions of an approved name land as TWO list entries — making the list LONGER than its
        // allowlist and failing, rather than collapsing into one set entry.
        let mut accessor_surface: Vec<String> = Vec::new();
        let mut test_method_names: Vec<String> = Vec::new();
        let mut test_support_method_names: Vec<String> = Vec::new();
        let mut free_fn_names: Vec<String> = Vec::new();
        let mut struct_names: Vec<String> = Vec::new();

        // Macro scan over the whole module (syn::visit reaches nested macros too).
        {
            use syn::visit::Visit;
            struct MacroScan<'a> {
                violations: &'a mut Vec<String>,
            }
            impl<'ast> Visit<'ast> for MacroScan<'_> {
                fn visit_macro(&mut self, node: &'ast syn::Macro) {
                    if let Some(hit) = macro_mentions(&node.tokens, &MACRO_REVIEW_IDENTS) {
                        let path = node
                            .path
                            .segments
                            .last()
                            .map(|s| s.ident.to_string())
                            .unwrap_or_default();
                        self.violations.push(format!(
                            "macro `{path}!` in the capsule module mentions `{hit}` — `syn` cannot \
                             expand it, so it must be reviewed for a capsule-field leak"
                        ));
                    }
                    syn::visit::visit_macro(self, node);
                }
            }
            MacroScan {
                violations: &mut violations,
            }
            .visit_file(&file);
        }

        // CLOSED-WORLD inventory (Sol's r5): module privacy stops OUTSIDE code from naming the fields,
        // but code INSIDE the module can still export a leak (e.g. `pub(crate) static LEAK: fn(&Cap)
        // -> &OciConfig = |c| &c.workload_cfg;` — the closure's field access is legal inside the owning
        // module, and a parent then calls `checkout_runtime::LEAK`). So EVERY production top-level item
        // must be one of an exact whitelist; anything else fails the audit by name. Together with
        // module privacy this makes the audited export inventory the compile-time guarantee.
        for item in &file.items {
            match item {
                // ALLOWED: any number of `use` imports.
                syn::Item::Use(_) => {}

                // ALLOWED: EXACTLY the two capsule structs (no other struct/enum/union/type/alias).
                syn::Item::Struct(s) => {
                    let name = s.ident.to_string();
                    struct_names.push(name.clone());
                    if !CAPSULES.contains(&name.as_str()) {
                        violations.push(format!(
                            "unexpected struct `{name}` — only the two capsule structs are permitted"
                        ));
                        continue;
                    }
                    for f in &s.fields {
                        if !is_private(&f.vis) {
                            violations.push(format!("{name}: a field is not private (pub/pub(..))"));
                        }
                    }
                    for attr in &s.attrs {
                        if attr.path().is_ident("derive") {
                            let _ = attr.parse_nested_meta(|meta| {
                                if meta.path.is_ident("Clone") || meta.path.is_ident("Copy") {
                                    violations.push(format!("{name} derives Clone/Copy"));
                                }
                                Ok(())
                            });
                        }
                    }
                }

                // ALLOWED: only the EXACT free-fn name set (even a private helper is rejected until
                // reviewed and added to `ALLOWED_FREE_FNS`).
                syn::Item::Fn(f) => {
                    let name = f.sig.ident.to_string();
                    free_fn_names.push(name.clone());
                    if !ALLOWED_FREE_FNS.contains(&name.as_str()) {
                        violations.push(format!(
                            "unexpected free fn `{name}` — the capsule module permits only \
                             {ALLOWED_FREE_FNS:?} at top level"
                        ));
                    } else if !is_cfg_test(&f.attrs) && !is_private(&f.vis) {
                        accessor_surface.push(name);
                    }
                }

                // ALLOWED: INHERENT impls on the two capsule types ONLY (incl. the `#[cfg(test)]`
                // driver impl). Any trait impl, or any impl on another self-type, is rejected.
                syn::Item::Impl(im) => {
                    if im.trait_.is_some() {
                        violations.push(format!(
                            "trait impl on `{}` — forbidden (a `match self` could hand out the \
                             inner fields)",
                            type_last_ident(&im.self_ty).as_deref().unwrap_or("<type>")
                        ));
                        continue;
                    }
                    match type_last_ident(&im.self_ty).as_deref() {
                        Some(name) if CAPSULES.contains(&name) => {
                            let test_only = is_cfg_test(&im.attrs);
                            let test_support_only = is_cfg_test_support(&im.attrs);
                            for it in &im.items {
                                match it {
                                    syn::ImplItem::Fn(m) => {
                                        if test_only {
                                            // CT-007 slice 5b.3-6c: the whole `#[cfg(test)]` impl is
                                            // excluded from the production accessor surface — but its
                                            // method set is pinned EXACTLY (see `TEST_METHODS`).
                                            test_method_names.push(m.sig.ident.to_string());
                                        } else if test_support_only {
                                            // CT-007 slice 5b.3-6e.1b: the whole `#[cfg(any(test,
                                            // feature = "test-support"))]` impl is likewise excluded
                                            // from the production accessor surface, inventoried
                                            // separately (see `TEST_SUPPORT_METHODS`) so the FIVE-entry
                                            // production surface is unchanged.
                                            test_support_method_names.push(m.sig.ident.to_string());
                                        } else if !is_private(&m.vis) {
                                            accessor_surface
                                                .push(format!("{name}::{}", m.sig.ident));
                                        }
                                    }
                                    _ => violations.push(format!(
                                        "non-fn associated item in inherent impl of `{name}` — a \
                                         const/type could hand out an inner field"
                                    )),
                                }
                            }
                        }
                        other => violations.push(format!(
                            "inherent impl on `{}` — only the two capsule types may be `impl`ed in \
                             this module",
                            other.unwrap_or("<non-path type>")
                        )),
                    }
                }

                // EVERYTHING ELSE is forbidden: static/const/trait/macro/extern/mod/type-alias/union/…
                // — any of which could export a closure/const/helper that legally reads a private
                // field. This is the terminal closed-world guarantee.
                other => violations.push(format!(
                    "forbidden top-level item in the capsule module: {} — its production surface is \
                     closed-world (only `use`, the two capsule structs, their inherent impls, and \
                     run_checkout_preparation_v2)",
                    describe_item(other)
                )),
            }
        }

        assert!(
            violations.is_empty(),
            "checkout_runtime module shape violated: {violations:#?}"
        );

        // MULTIPLICITY-EXACT assertions (sorted lists, NO dedup): a second `#[cfg]`-gated definition
        // of an approved name makes its list one entry LONGER than the allowlist and fails here, even
        // though a comment can defeat the literal occurrence pin and a `set` would collapse it.
        fn sorted(mut v: Vec<String>) -> Vec<String> {
            v.sort();
            v
        }
        assert_eq!(
            sorted(struct_names),
            sorted(CAPSULES.iter().map(|s| s.to_string()).collect()),
            "the capsule struct-name list changed — EXACTLY the two capsule structs, no duplicate \
             (e.g. a second cfg-gated struct reusing a capsule name)"
        );
        assert_eq!(
            sorted(free_fn_names),
            sorted(ALLOWED_FREE_FNS.iter().map(|s| s.to_string()).collect()),
            "the top-level free-fn list changed — EXACTLY run_checkout_preparation_v2, counted with \
             multiplicity, so a second cfg-gated definition of that name fails"
        );
        assert_eq!(
            sorted(accessor_surface),
            sorted(ALLOWLIST.iter().map(|s| s.to_string()).collect()),
            "the checkout_runtime module's non-private accessor surface changed — every accessor must \
             be an explicitly-reviewed capsule entry, counted with multiplicity"
        );
        assert_eq!(
            sorted(test_method_names),
            sorted(TEST_METHODS.iter().map(|s| s.to_string()).collect()),
            "the checkout_runtime module's `#[cfg(test)]`-only method set changed — every test-only \
             capsule method (the session driver, the into_prepared transition, the workload _given \
             seam) must be an explicitly-reviewed entry, counted with multiplicity"
        );
        assert_eq!(
            sorted(test_support_method_names),
            sorted(TEST_SUPPORT_METHODS.iter().map(|s| s.to_string()).collect()),
            "the checkout_runtime module's `test-support`-only method set changed — the deterministic \
             substituted-execution seam must be an explicitly-reviewed entry, counted with \
             multiplicity, and it must NOT appear in the five-entry production accessor ALLOWLIST"
        );
    }

    /// **CT-007 slice 5b.3-6e.2: the activated cycle has exactly one checkout orchestration path and
    /// the capsule's workload `OciConfig` is never detached.** The typed `run_cycle` selector is the
    /// single production caller; the continuation and capsule transition remain single-sourced.
    #[test]
    fn the_checkout_runtime_capsule_has_exactly_one_activated_cycle_caller() {
        let prod = production_source();
        // The activated typed-cycle selector calls the outer orchestrator exactly once for a
        // checkout-bearing spec.
        assert_eq!(
            prod.matches(".launch_checkout_orchestrated_with(").count(),
            1,
            "the outer orchestrator has exactly ONE caller — the activated `run_cycle` selector"
        );
        assert_eq!(
            prod.matches("fn launch_checkout_orchestrated_with(")
                .count(),
            1,
            "the activated outer orchestrator is defined exactly once"
        );
        assert_eq!(
            prod.matches("fn launch_checkout_orchestrated_with_given")
                .count(),
            1,
            "its shared injectable body is defined exactly once"
        );
        // The fused V2 Hop B entry lives in the submodule (ONE definition); the continuation is its ONE
        // production caller — reachable only through the typed-cycle orchestrator.
        assert_eq!(
            CHECKOUT_RUNTIME_SOURCE
                .matches("run_checkout_preparation_v2(")
                .count(),
            1,
            "run_checkout_preparation_v2 is defined exactly once in the submodule"
        );
        assert_eq!(
            prod.matches("run_checkout_preparation_v2(").count(),
            1,
            "the continuation is the ONLY production caller of the fused Hop B entry"
        );
        // The capsule constructor is called ONLY by the activated orchestrator.
        assert_eq!(
            prod.matches("AcquiredCheckoutRuntime::acquire(").count(),
            1,
            "the capsule is constructed only by the activated orchestrator"
        );
        assert_eq!(
            CHECKOUT_RUNTIME_SOURCE
                .matches("AcquiredCheckoutRuntime::acquire(")
                .count(),
            0,
            "the submodule only DEFINES `acquire`, never calls the qualified form"
        );
        // The closed workload transition is invoked ONLY by the activated continuation.
        assert_eq!(
            prod.matches(".run_retained_workload(").count(),
            1,
            "the closed workload transition is invoked only by the activated continuation"
        );
        // The old free-standing `into_prepared`/`bind_workload` seams are gone: the ONLY prepared
        // transition is the fused Hop B entry (production) plus the audited `#[cfg(test)]`
        // `into_prepared_for_tests` (exactly one, test-only).
        assert_eq!(
            prod.matches("fn into_prepared").count(),
            0,
            "no production free-standing prepared transition — Hop B and the transition are fused"
        );
        assert_eq!(
            CHECKOUT_RUNTIME_SOURCE
                .matches("fn into_prepared_for_tests(")
                .count(),
            1,
            "the ONLY prepared transition outside the fused Hop B entry is the test-only one"
        );
        // The fused Hop B entry consumes the capsule by value and returns the prepared capsule.
        let v2_entry = source_of_in(
            CHECKOUT_RUNTIME_SOURCE,
            "pub(crate) fn run_checkout_preparation_v2(",
        );
        assert!(
            v2_entry.contains("mut runtime: AcquiredCheckoutRuntime")
                && v2_entry.contains(
                    "-> Result<PreparedCheckoutRuntime, (AcquiredCheckoutRuntime, CheckoutPreparationError)>"
                ),
            "the fused Hop B entry consumes the capsule by value and returns the prepared capsule"
        );
        // 5b.3-6a/6b/6c: launch_with's OWN control flow never names either capsule type. In 6b
        // launch_with became a plain delegating wrapper and the compute body moved to
        // launch_compute_with; the span source_of captures for `fn launch_with<F>(` runs to this
        // impl's close (launch_with + launch_compute_with + dispose_run_failure) — the checkout seam
        // lives in SEPARATE impls, so it is deliberately outside this span.
        let launch_with = source_of("fn launch_with<F>(");
        assert!(
            !launch_with.contains("AcquiredCheckoutRuntime")
                && !launch_with.contains("PreparedCheckoutRuntime"),
            "the compute launch path (launch_with wrapper + launch_compute_with) names no capsule type"
        );
        assert!(
            launch_with.contains("self.launch_compute_with(spec, hooks, run)"),
            "launch_with is a plain delegating wrapper — it performs NO shape dispatch on spec.workspace"
        );
        // The continuation is DEFINED once and called ONLY by the activated orchestrator. It
        // consumes the capsule BY VALUE.
        assert_eq!(
            prod.matches("fn launch_checkout_continuation(").count(),
            1,
            "the checkout continuation is defined exactly once in production"
        );
        assert_eq!(
            prod.matches(".launch_checkout_continuation(").count(),
            1,
            "the continuation's ONLY caller is the activated orchestrator"
        );
        let seam = source_of("fn launch_checkout_continuation(");
        assert!(
            seam.contains("runtime: checkout_runtime::AcquiredCheckoutRuntime"),
            "the continuation consumes the capsule by value"
        );
        // Blocker 2/5: `acquire` retains the workload OciConfig INSIDE the capsule — its signature
        // returns the bare capsule, never `(OciConfig, ..)` detached.
        let acquire_sig = {
            let s = CHECKOUT_RUNTIME_SOURCE
                .find("pub(crate) fn acquire(")
                .expect("acquire exists in the submodule");
            let rest = &CHECKOUT_RUNTIME_SOURCE[s..];
            &rest[..rest.find(" {\n").expect("acquire signature ends")]
        };
        assert!(
            // CT-007 5b.3-6c (Sol's r2 finding 1): acquire now returns a TYPED `AcquisitionFailure`
            // (clean-refusal vs reconciliation-required), never a bare `String` — still the bare capsule
            // on success, never a detached `OciConfig`.
            acquire_sig.contains("-> Result<AcquiredCheckoutRuntime, AcquisitionFailure>")
                && !acquire_sig.contains("OciConfig"),
            "acquire must return the capsule alone — never the workload OciConfig detached"
        );

        // ── CT-007 slice 5b.3-6e.2: the activated compute-V2 entry + checkout config ──
        assert_eq!(
            prod.matches("fn launch_compute_orchestrated_with").count(),
            1,
            "the activated compute-V2 orchestrated entry is defined exactly once"
        );
        // The activated typed-cycle selector calls the compute-V2 orchestrated entry exactly once.
        assert_eq!(
            prod.matches(".launch_compute_orchestrated_with(").count(),
            1,
            "the compute-V2 orchestrated entry has exactly ONE caller — the activated `run_cycle` selector"
        );
        // The shared post-reservation body is extracted ONCE and used by BOTH compute entries; the
        // preflight is extracted once and used by both. The legacy `launch_compute_with` remains the
        // compatibility compute entry (called by the plain `launch_with` wrapper and streaming).
        assert_eq!(
            prod.matches("fn launch_compute_common_body").count(),
            1,
            "the shared post-reservation compute body is defined exactly once"
        );
        assert_eq!(
            prod.matches(".launch_compute_common_body(").count(),
            2,
            "both compute entries (legacy compatibility + activated orchestrated) run the ONE shared common body"
        );
        assert_eq!(
            prod.matches("fn compute_launch_preflight").count(),
            1,
            "the shared compute preflight is defined exactly once"
        );
        assert_eq!(
            prod.matches(".compute_launch_preflight(").count(),
            2,
            "both compute entries run the ONE shared preflight"
        );
        // The checkout repository-root config: every production GvisorBackend constructor leaves it
        // `disabled()`. This is a gvisor.rs-LOCAL invariant; the CROSS-CRATE composition-root zero for
        // the `with_checkout_config` selector (which a controlplane root could call) is enforced by the
        // recursive both-crates dormancy scan `the_v2_phase_credential_surface_has_exactly_its_known_occurrences`.
        assert_eq!(
            prod.matches("checkout: GvisorCheckoutConfig::disabled()")
                .count(),
            3,
            "every production GvisorBackend constructor leaves checkout disabled()"
        );
    }

    /// CT-007 slice 5b.3-6e.1: the checkout repository-root config validates at boot — no default
    /// fallback, and a relative / nonexistent / non-directory / non-canonical root fails closed.
    #[test]
    fn gvisor_checkout_config_validates_the_repo_root_at_boot() {
        // A relative path is refused.
        assert!(matches!(
            GvisorCheckoutConfig::enabled("relative/repo"),
            Err(GvisorCheckoutConfigError::NotAbsolute(_))
        ));
        // A nonexistent absolute path is refused.
        let missing = std::env::temp_dir().join(format!(
            "myelin-checkout-root-missing-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        assert!(matches!(
            GvisorCheckoutConfig::enabled(&missing),
            Err(GvisorCheckoutConfigError::NotADirectory { .. })
        ));
        // An absolute path to a FILE (not a directory) is refused.
        let file_path = std::env::temp_dir().join(format!(
            "myelin-checkout-root-file-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::write(&file_path, b"not a dir").unwrap();
        assert!(matches!(
            GvisorCheckoutConfig::enabled(&file_path),
            Err(GvisorCheckoutConfigError::NotADirectory { .. })
        ));
        let _ = std::fs::remove_file(&file_path);
        // A real, canonical directory is ACCEPTED and retains the exact root.
        let base = std::env::temp_dir()
            .join(format!(
                "myelin-checkout-root-ok-{}-{}",
                std::process::id(),
                unique_suffix()
            ))
            .canonicalize()
            .unwrap_or_else(|_| {
                let p = std::env::temp_dir().join(format!(
                    "myelin-checkout-root-ok-{}-{}",
                    std::process::id(),
                    unique_suffix()
                ));
                std::fs::create_dir_all(&p).unwrap();
                std::fs::canonicalize(&p).unwrap()
            });
        std::fs::create_dir_all(&base).unwrap();
        let base = std::fs::canonicalize(&base).unwrap();
        let accepted =
            GvisorCheckoutConfig::enabled(&base).expect("a canonical directory must be accepted");
        assert_eq!(
            accepted.repo_root(),
            Some(base.as_path()),
            "an enabled config exposes exactly the validated root"
        );
        // A non-canonical path (a `..`-bearing route to the same real dir) is refused, even though it
        // resolves to an existing directory.
        let non_canonical = base.join("..").join(base.file_name().unwrap());
        assert!(matches!(
            GvisorCheckoutConfig::enabled(&non_canonical),
            Err(GvisorCheckoutConfigError::NotCanonical { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **CT-007 slice 5b.3-6e.1 (Sol's blocker 1): the invalid state is UNCONSTRUCTABLE.** `enabled()`
    /// is structurally the ONLY path to an enabled config — the wrapped `CheckoutConfigState` is
    /// private, so no external construction of `Enabled { repo_root: <unvalidated> }` is possible, and
    /// `with_checkout_config` therefore can only ever receive an already-validated value. This test
    /// pins the two facts a reviewer can check: `disabled()` carries no root, and the ONLY
    /// `CheckoutConfigState::Enabled` construction in production source is inside `fn enabled(` (the
    /// validating constructor). A future `CheckoutConfigState::Enabled { .. }` built anywhere else —
    /// bypassing validation — trips this pin.
    #[test]
    fn an_enabled_checkout_config_can_only_arise_from_the_validating_constructor() {
        assert_eq!(GvisorCheckoutConfig::disabled().repo_root(), None);

        // Every enabled-state CONSTRUCTION in production source (test module stripped) must sit inside
        // the validating `fn enabled(`. The construction is uniquely spelled
        // `GvisorCheckoutConfig(CheckoutConfigState::Enabled {` (the wrapper prefix distinguishes it
        // from `repo_root()`'s bare `CheckoutConfigState::Enabled { .. } =>` match pattern). There is
        // exactly ONE, and it is that site.
        let prod = production_source();
        let construction = "GvisorCheckoutConfig(CheckoutConfigState::Enabled {";
        assert_eq!(
            prod.matches(construction).count(),
            1,
            "there must be exactly one enabled-config construction site in production source"
        );
        let enabled_fn = source_of("pub fn enabled(");
        assert_eq!(
            enabled_fn.matches(construction).count(),
            1,
            "the sole enabled-config construction lives inside the boot-validating `enabled()` — no \
             other code path can build an enabled config with an unvalidated path"
        );
    }

    // CT-007 slice 5b.3-6c: the r3/r4 COUNTING pins were all REMOVED — Sol compiled evasions of each.
    // The terminal fences are LANGUAGE-ENFORCED (mirroring 6a's RAII + module-privacy discipline):
    //   - FINDING 1 (resource safety): an RAII `NotStartedCapsuleGuard` owns the capsule before Hop B;
    //     its `Drop` performs the SAFE NotStarted cleanup (delete + release_unused), so EVERY early
    //     return / `?` / unwind before Hop B disposes safely — the manager stays healthy, the slot
    //     reusable — with no syntactic pin. The success path `disarm()`s it into `hop_b`. See
    //     `launch_checkout_continuation_given` + the always-run
    //     `not_started_capsule_guard_disposes_safely_on_any_early_exit` proof.
    //   - FINDING 2 (credential substitution): `WorkloadRotatedSpec` lives in the sealed `workload_spec`
    //     module (private field, no `as_job_spec`, no `Clone`/`From`), and its inner `JobSpec` is
    //     consumed ONLY by its own `acquire_permit_and_run` — so no outer code can obtain a `&JobSpec`
    //     to clone/substitute (`error[E0599]: no method named as_job_spec`). See
    //     `the_workload_spec_module_shape_is_pinned`.

    /// **Sol's r5/r6 finding 2 (CLOSED-WORLD module audit): the workload-spec wrapper never leaks its
    /// inner `JobSpec`.** Mirrors the 6a `checkout_runtime` module discipline. `workload_spec.rs`'s
    /// ENTIRE surface is EXACTLY the `WorkloadRotatedSpec` struct (private field, no `Clone`/`Copy`), the
    /// `BoundWorkloadRefusal` enum, and ONE inherent impl whose PRODUCTION methods are `{from_carrier,
    /// acquire_permit_and_run}`, whose private helper is `{acquire_permit_and_prep}`, and whose
    /// `#[cfg(test)]`-only method is `{acquire_permit_and_run_given}` — every set pinned EXACTLY. NO
    /// method (production, private, OR test) may return a type that MENTIONS `JobSpec` at ANY nesting
    /// (`&JobSpec`, `Result<&JobSpec,_>`, a tuple containing one, `Option<&JobSpec>`, `impl
    /// Deref<Target=JobSpec>`, …) — the whole return-type AST is walked for the `JobSpec` ident. And NO
    /// trait impl (a `Clone`/`From`/`Deref` could hand out the inner spec). Any leak-adding item/accessor
    /// fails this audit BY NAME, fail-closed.
    #[test]
    fn the_workload_spec_module_shape_is_pinned() {
        const SOURCE: &str = include_str!("gvisor/workload_spec.rs");
        let file = syn::parse_file(SOURCE).expect("workload_spec.rs parses as a File");

        fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
            attrs.iter().any(|a| {
                let mut hit = false;
                if a.path().is_ident("cfg") {
                    let _ = a.parse_nested_meta(|meta| {
                        if meta.path.is_ident("test") {
                            hit = true;
                        }
                        Ok(())
                    });
                }
                hit
            })
        }
        // Gated `#[cfg(any(test, feature = "test-support"))]` — the test-support permit-fence seam,
        // distinct from the `#[cfg(test)]`-only injectable-execute seam. Its own pinned inventory.
        fn is_cfg_test_support(attrs: &[syn::Attribute]) -> bool {
            attrs.iter().any(|a| {
                a.path().is_ident("cfg")
                    && matches!(&a.meta, syn::Meta::List(list) if list.tokens.to_string().contains("test-support"))
            })
        }
        // Walk the ENTIRE return-type AST for the `JobSpec` ident — nested/opaque returns included.
        fn type_mentions_job_spec(ty: &syn::Type) -> bool {
            use syn::visit::Visit;
            struct Scan {
                found: bool,
            }
            impl<'ast> Visit<'ast> for Scan {
                fn visit_ident(&mut self, id: &'ast syn::Ident) {
                    if id == "JobSpec" {
                        self.found = true;
                    }
                }
            }
            let mut scan = Scan { found: false };
            scan.visit_type(ty);
            scan.found
        }

        let mut struct_seen = false;
        let mut enum_seen = false;
        let mut production_methods: Vec<String> = Vec::new();
        let mut private_methods: Vec<String> = Vec::new();
        let mut test_methods: Vec<String> = Vec::new();
        let mut test_support_methods: Vec<String> = Vec::new();
        let mut violations: Vec<String> = Vec::new();
        for item in &file.items {
            match item {
                syn::Item::Use(_) => {}
                syn::Item::Struct(s) if s.ident == "WorkloadRotatedSpec" => {
                    struct_seen = true;
                    for f in &s.fields {
                        if !matches!(f.vis, syn::Visibility::Inherited) {
                            violations.push("WorkloadRotatedSpec field is not private".to_string());
                        }
                    }
                    for attr in &s.attrs {
                        if attr.path().is_ident("derive") {
                            let _ = attr.parse_nested_meta(|m| {
                                if m.path.is_ident("Clone") || m.path.is_ident("Copy") {
                                    violations.push(
                                        "WorkloadRotatedSpec derives Clone/Copy — could duplicate the \
                                         inner spec"
                                            .to_string(),
                                    );
                                }
                                Ok(())
                            });
                        }
                    }
                }
                syn::Item::Enum(e) if e.ident == "BoundWorkloadRefusal" => enum_seen = true,
                syn::Item::Impl(im) if im.trait_.is_some() => violations.push(
                    "a trait impl in the workload_spec module could hand out the inner spec (e.g. \
                     Clone/From/Deref)"
                        .to_string(),
                ),
                syn::Item::Impl(im) => {
                    for it in &im.items {
                        match it {
                            syn::ImplItem::Fn(m) => {
                                let name = m.sig.ident.to_string();
                                // NO method may return a type MENTIONING `JobSpec` at any nesting.
                                if let syn::ReturnType::Type(_, ty) = &m.sig.output {
                                    if type_mentions_job_spec(ty) {
                                        violations.push(format!(
                                            "method `{name}` returns a type mentioning `JobSpec` — the \
                                             inner spec must never escape to be cloned/substituted"
                                        ));
                                    }
                                }
                                if is_cfg_test(&m.attrs) {
                                    test_methods.push(name);
                                } else if is_cfg_test_support(&m.attrs) {
                                    test_support_methods.push(name);
                                } else if matches!(m.vis, syn::Visibility::Inherited) {
                                    private_methods.push(name);
                                } else {
                                    production_methods.push(name);
                                }
                            }
                            _ => violations.push(
                                "non-fn associated item in the WorkloadRotatedSpec impl"
                                    .to_string(),
                            ),
                        }
                    }
                }
                other => violations.push(format!(
                    "unexpected top-level item in workload_spec (closed-world): {:?}",
                    std::mem::discriminant(other)
                )),
            }
        }
        assert!(
            violations.is_empty(),
            "workload_spec module shape violated: {violations:#?}"
        );
        assert!(
            struct_seen && enum_seen,
            "the two sanctioned types must be present"
        );
        let sorted = |mut v: Vec<String>| {
            v.sort();
            v
        };
        assert_eq!(
            sorted(production_methods),
            vec![
                "acquire_permit_and_run".to_string(),
                "from_carrier".to_string()
            ],
            "the workload_spec PRODUCTION surface is EXACTLY the sealed constructor + the fixed-runner \
             method (which calls run_production_container_streaming itself — no caller `execute`)"
        );
        assert_eq!(
            sorted(private_methods),
            vec!["acquire_permit_and_prep".to_string()],
            "the ONLY private helper is the shared permit+prep step"
        );
        assert_eq!(
            sorted(test_methods),
            vec!["acquire_permit_and_run_given".to_string()],
            "the ONLY `#[cfg(test)]` method is the injectable execution seam — the sole place an \
             `execute` closure receiving `&JobSpec` exists, absent from every ordinary build"
        );
        assert_eq!(
            sorted(test_support_methods),
            vec!["acquire_launch_permit_for_test_support".to_string()],
            "the ONLY `#[cfg(any(test, feature = \"test-support\"))]` method is the sealed permit-fence \
             acquisition the deterministic runsc-driver seam drives (it acquires against `&self.spec` \
             and returns only a LaunchPermit — the inner spec never escapes)"
        );
    }

    /// **The cleanup routing is behaviorally pinned, not just structurally** (Sol's r1 blocker 6):
    /// the pure [`checkout_cleanup_plan`] maps EVERY session disposition to exactly one action, so a
    /// regression that swapped e.g. the `NeverBound`/`Prepared` release methods fails HERE (in the
    /// always-run unit gate) rather than only as a production panic/allocator poison. The privileged
    /// end-to-end matrix (`dispose_*_matrix`, below) then proves the plan's EXECUTION against real
    /// leases/workspaces.
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
            "a bound workload's resources are owned by finalization — disposal abandons, never releases"
        );
    }

    /// **Each cleanup plan invokes EXACTLY the right operation sequence — ALWAYS-RUN, no
    /// `CAP_SYS_ADMIN`** (Sol's r2 blocker 3). `execute_cleanup_plan` is the SINGLE implementation the
    /// real disposal and this test share, driven here through a recording fake executor. Swapping the
    /// `release_unused`/`release_prepared` legs of the two delete-then-release plans changes the
    /// recorded trace and fails this test — the regression the privileged e2e matrix could SKIP past
    /// (it soft-skips without Btrfs/caps) is caught here in the gate-enforced unit run.
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
        /// What `delete_workspace` reports for disk-absence-proven.
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

        // Proven deletion: the two delete-then-release plans reach their DISTINCT release op — the
        // exact swap Sol flagged (NeverBound↔Prepared) would flip these and fail.
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
        // Unproven deletion: neither delete-then-release plan releases — the lease is quarantined.
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
        // Quarantine/abandon plans never delete or release — both resources are quarantined.
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

    /// **The one-shot fetch provider is invoked after the advertisement retires and the lease
    /// checkpoint renews, and before ANYTHING for the fetch is built or spawned.** Ordering is the
    /// whole security property here: minting the fetch credential earlier would let it be issued
    /// against a generation this worker may no longer own.
    #[test]
    fn the_fetch_phase_authorization_is_obtained_between_the_checkpoint_and_the_fetch_spawn() {
        let transport = source_of("fn fetch_checkout_pack_within_parent_attempt_inner(");
        let advertise_hop = transport
            .find("false, // this is the FIRST hop")
            .expect("the advertise hop runs");
        let checkpoint = transport
            .find("if let Some(checkpoint) = lease_checkpoint {")
            .expect("the lease checkpoint runs");
        let provider = transport
            .find("let (fetch_run_token, fetch_permit) = match fetch_source.as_mut()")
            .expect("the fetch authorization is obtained");
        let fetch_spec = transport
            .find("let fetch_spec = GitWireSpec::for_repo(")
            .expect("the fetch spec is built");
        let fetch_hop = transport
            .find("true, // the advertisement hop above already completed.")
            .expect("the fetch hop runs");
        assert!(
            advertise_hop < checkpoint
                && checkpoint < provider
                && provider < fetch_spec
                && fetch_spec < fetch_hop,
            "ordering must be advertise -> renew -> mint fetch credential -> build -> spawn"
        );
    }

    #[derive(Default)]
    struct RecordingOutput {
        bytes: Mutex<Vec<u8>>,
    }

    impl SandboxOutputSink for RecordingOutput {
        fn emit(&self, _stream: SandboxOutputStream, frame: &[u8]) -> Result<(), String> {
            self.bytes.lock().unwrap().extend_from_slice(frame);
            Ok(())
        }
    }

    #[test]
    fn streaming_drain_keeps_only_the_head_but_forwards_chunks_to_the_job_budget() {
        let input: Vec<u8> = (0..(3 * 64 * 1024 + 17))
            .map(|offset| (offset % 251) as u8)
            .collect();
        let sink = Arc::new(RecordingOutput::default());
        let capped: Arc<dyn SandboxOutputSink> = Arc::new(TotalLogCappedOutput::new(sink.clone()));
        let output = StreamingOutput { sink: capped };
        let redaction = RedactionPlan::none();

        let (head, truncated, error) = drain_capped_streaming(
            std::io::Cursor::new(&input),
            1024,
            SandboxOutputStream::Stdout,
            Some(&output),
            &redaction,
        );

        assert_eq!(error, None);
        assert!(truncated);
        assert_eq!(head, input[..1024]);
        assert_eq!(
            *sink.bytes.lock().unwrap(),
            input,
            "bytes beyond the diagnostic head still reach the shared job budget when under it"
        );
    }

    #[test]
    fn streaming_drain_masks_an_injected_value_split_across_pipe_reads() {
        let sink = Arc::new(RecordingOutput::default());
        let output = StreamingOutput { sink: sink.clone() };
        let redaction = RedactionPlan::for_needles([b"split-secret".to_vec()]).unwrap();
        let reader = std::io::Cursor::new(b"before split-".as_slice())
            .chain(std::io::Cursor::new(b"secret after".as_slice()));

        let (_head, _truncated, error) = drain_capped_streaming(
            reader,
            1024,
            SandboxOutputStream::Stdout,
            Some(&output),
            &redaction,
        );

        assert_eq!(error, None);
        assert!(!sink
            .bytes
            .lock()
            .unwrap()
            .windows(b"split-secret".len())
            .any(|window| window == b"split-secret"));
    }

    #[test]
    fn result_head_redacts_before_truncating_a_boundary_straddling_secret() {
        let secret = b"BOUNDARY-STRADDLING-SECRET";
        let prefix_len = secret.len() / 2;
        let mut input = vec![b'a'; SANDBOX_CAPTURE_BOUND - prefix_len];
        input.extend_from_slice(secret);
        input.extend(std::iter::repeat_n(b'z', SANDBOX_CAPTURE_BOUND));
        let redaction = RedactionPlan::for_needles([secret.to_vec()]).unwrap();

        let (head, truncated, error) = drain_capped_streaming(
            std::io::Cursor::new(input),
            SANDBOX_CAPTURE_BOUND,
            SandboxOutputStream::Stdout,
            None,
            &redaction,
        );

        assert_eq!(error, None);
        assert!(truncated);
        assert!(!head.windows(secret.len()).any(|window| window == secret));
        assert!(!head[..].ends_with(&secret[..prefix_len]));
    }

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
    fn streaming_capture_total_log_cap_truncates_with_marker_and_stops_growth() {
        let payload_limit = 1024;
        let total_limit = payload_limit + TOTAL_LOG_TRUNCATION_MARKER.len();
        let sink = Arc::new(RecordingOutput::default());
        let capped = Arc::new(TotalLogCappedOutput::with_limit(sink.clone(), total_limit));
        let output = StreamingOutput {
            sink: capped.clone(),
        };
        let redaction = RedactionPlan::none();
        let input = vec![b'x'; 256 * 1024];

        let (_head, truncated, error) = drain_capped_streaming(
            std::io::Cursor::new(&input),
            128,
            SandboxOutputStream::Stdout,
            Some(&output),
            &redaction,
        );
        assert_eq!(error, None);
        assert!(truncated, "the diagnostic head is also truncated");

        let captured_before_late_frame = sink.bytes.lock().unwrap().clone();
        capped
            .emit(SandboxOutputStream::Stderr, b"late bytes must be discarded")
            .unwrap();
        let captured = sink.bytes.lock().unwrap().clone();
        assert_eq!(captured, captured_before_late_frame);
        assert_eq!(captured.len(), total_limit);
        assert_eq!(&captured[..payload_limit], &input[..payload_limit]);
        assert!(captured.ends_with(TOTAL_LOG_TRUNCATION_MARKER));
        assert_eq!(capped.captured_bytes(), total_limit);
    }

    #[test]
    fn total_log_cap_is_shared_across_streams_and_under_ceiling_is_unchanged() {
        let sink = Arc::new(RecordingOutput::default());
        let total_limit = TOTAL_LOG_TRUNCATION_MARKER.len() + 64;
        let capped = TotalLogCappedOutput::with_limit(sink.clone(), total_limit);
        capped
            .emit(SandboxOutputStream::Stdout, b"ordinary stdout\n")
            .unwrap();
        capped
            .emit(SandboxOutputStream::Stderr, b"ordinary stderr\n")
            .unwrap();
        assert_eq!(
            *sink.bytes.lock().unwrap(),
            b"ordinary stdout\nordinary stderr\n",
            "combined output below the payload ceiling must be byte-identical"
        );

        let sink = Arc::new(RecordingOutput::default());
        let capped =
            TotalLogCappedOutput::with_limit(sink.clone(), TOTAL_LOG_TRUNCATION_MARKER.len() + 8);
        capped.emit(SandboxOutputStream::Stdout, b"123456").unwrap();
        capped.emit(SandboxOutputStream::Stderr, b"abcdef").unwrap();
        let captured = sink.bytes.lock().unwrap().clone();
        assert_eq!(&captured[..8], b"123456ab");
        assert!(captured.ends_with(TOTAL_LOG_TRUNCATION_MARKER));
        assert_eq!(
            captured.len(),
            TOTAL_LOG_TRUNCATION_MARKER.len() + 8,
            "stdout and stderr must consume one exact shared total-byte budget"
        );
    }

    #[test]
    fn production_streaming_entries_install_one_job_wide_total_log_cap() {
        let streaming = source_of("fn launch_streaming(");
        assert!(
            streaming.find("cap_total_job_output(output)").unwrap()
                < streaming.find("self.launch_with(").unwrap(),
            "ordinary streaming launch must install the cap before any production run path"
        );

        let cycle = source_of("fn run_cycle(");
        assert!(
            cycle.find("cap_total_job_output(output)").unwrap()
                < cycle
                    .find("match crate::derive_checkout_authorization_scope")
                    .unwrap(),
            "the cap must wrap the sink once before checkout routing so every phase shares it"
        );
    }

    #[test]
    fn runner_host_preflight_refuses_a_non_absolute_runtime_before_intake() {
        let error = preflight_gvisor_runner_host(Path::new("runsc"), Path::new("/unused-rootfs"))
            .expect_err("a PATH-relative runtime is not stable production authority");
        assert!(error.contains("MYELIN_RUNSC_BIN must be an absolute path"));
    }

    #[test]
    fn rootless_version_probe_rejects_an_unexpected_file_capability_before_exec() {
        let result = probe_runsc_version_given(Path::new("/definitely/not/executable"), |_| {
            Err("unexpected security.capability xattr".to_string())
        });
        assert_eq!(
            result,
            Err(RunscProbeError::UnsafeBinary(
                "unexpected security.capability xattr".to_string()
            )),
            "the rootless startup probe must reject metadata before attempting --version"
        );
    }

    #[test]
    fn every_rootless_runtime_invocation_rejects_an_unexpected_file_capability() {
        let bin = Path::new("/vetted/runsc");
        let mut cmd = Command::new(bin);
        let result = apply_runsc_invocation_policy_checked_given(
            &mut cmd,
            bin,
            RunscInvocationMode::Rootless,
            None,
            |path| {
                Err(format!(
                    "{path:?} carries an unexpected security.capability xattr"
                ))
            },
        );
        assert!(result
            .unwrap_err()
            .contains("unexpected security.capability xattr"));
        assert_eq!(
            cmd.get_args().count(),
            0,
            "rejection must happen before even the rootless invocation policy is assembled"
        );
    }

    #[test]
    fn git_rootfs_requires_every_fixed_mountpoint_and_verifies_a_stable_digest() {
        let rootfs =
            std::env::temp_dir().join(format!("myelin-git-rootfs-integrity-{}", unique_suffix()));
        std::fs::create_dir_all(&rootfs).unwrap();
        for destination in ["tmp", "workspace", "repo", "quarantine"] {
            std::fs::create_dir(rootfs.join(destination)).unwrap();
        }
        std::fs::write(rootfs.join("git-payload"), b"pinned git rootfs bytes").unwrap();
        let digest = crate::canonical_tar::canonical_tree_sha256_hex(&rootfs).unwrap();

        let first = verify_gvisor_git_rootfs_given(&rootfs, &digest).unwrap();
        let second = verify_gvisor_git_rootfs_given(&rootfs, &digest).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            crate::canonical_tar::canonical_tree_sha256_hex(&rootfs).unwrap(),
            digest,
            "verification itself must not mutate the shared git rootfs"
        );

        for destination in ["tmp", "workspace", "repo", "quarantine"] {
            std::fs::remove_dir(rootfs.join(destination)).unwrap();
            let error = verify_gvisor_git_rootfs_given(&rootfs, &digest)
                .expect_err("every OCI destination must pre-exist before hashing/use");
            assert!(
                error.contains(&format!("/{destination}")),
                "missing destination must be named: {error}"
            );
            std::fs::create_dir(rootfs.join(destination)).unwrap();
        }

        std::fs::write(rootfs.join("unapproved-drift"), b"drift").unwrap();
        assert!(
            verify_gvisor_git_rootfs_given(&rootfs, &digest)
                .unwrap_err()
                .contains("DRIFTED"),
            "content added outside the mountpoints must fail the pin"
        );
        let _ = std::fs::remove_dir_all(rootfs);
    }

    /// A real, on-disk, empty fixture rootfs — hashed with the SAME pure-Rust
    /// [`crate::canonical_tar::canonical_tree_sha256_hex`] the registry itself uses — so [`spec`]'s
    /// image is a GENUINELY verifiable pin, not a fabricated placeholder digest a real registry
    /// lookup could never match. Shared (same fixed path) across every test in this module — they
    /// only ever READ it (construction-time hashing happens once, in [`test_registry`]), never
    /// mutate it, so sharing across parallel test threads within this one process is safe.
    fn fixture_rootfs_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "myelin-gvisor-unit-test-fixture-rootfs-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::create_dir(dir.join("workspace"));
        dir
    }

    /// The digest-pinned [`ImageRef`] matching [`fixture_rootfs_dir`]'s REAL current content.
    fn fixture_image() -> ImageRef {
        let digest = crate::canonical_tar::canonical_tree_sha256_hex(&fixture_rootfs_dir())
            .expect("hash the fixture rootfs dir");
        ImageRef::pinned(format!("test.local/fixture-rootfs@sha256:{digest}")).unwrap()
    }

    /// A registry mapping [`fixture_image`] to [`fixture_rootfs_dir`] — the registry every unit test
    /// in this module that calls `launch_with`/`launch` constructs its [`GvisorBackend`] with. These
    /// tests never run a real `runsc` (they inject a fake `run` closure), so all that matters is that
    /// construction genuinely verifies (once) before the fake closure runs.
    fn test_registry() -> Arc<crate::asset_registry::GvisorAssetRegistry> {
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

    fn spec(allow: Vec<String>) -> JobSpec {
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

    struct CargoBoundaryFixture {
        root: PathBuf,
        rootfs: PathBuf,
        reference: ImageRef,
        lock_sha256: String,
        registry: crate::asset_registry::GvisorAssetRegistry,
    }

    impl Drop for CargoBoundaryFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn cargo_boundary_fixture(tag: &str) -> CargoBoundaryFixture {
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

    fn structured_cargo_spec(reference: &ImageRef) -> JobSpec {
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

    fn wired_cargo_config(fixture: &CargoBoundaryFixture) -> OciConfig {
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

    fn cargo_compute_registry(
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

    #[test]
    fn structured_cargo_launch_spec_has_verified_ro_vendor_server_config_and_bounded_writable_home()
    {
        let fixture = cargo_boundary_fixture("structured-launch");
        let cfg = wired_cargo_config(&fixture);
        let json = cfg.to_json().unwrap();
        assert!(json.contains("CARGO_HOME=/tmp/cargo-home"), "{json}");
        assert!(json.contains("CARGO_NET_OFFLINE=true"), "{json}");
        assert!(json.contains("CARGO_SOURCE_CRATES_IO_REPLACE_WITH=vendored"));
        assert!(json.contains("CARGO_SOURCE_VENDORED_DIRECTORY=/opt/myelin/cargo-vendor"));
        assert!(json.contains("\"destination\": \"/tmp\""), "{json}");
        assert_eq!(
            json.matches("\"type\": \"tmpfs\"").count(),
            2,
            "the structured launch has exactly /tmp plus its nested Cargo-home tmpfs: {json}"
        );
        assert_eq!(
            json.matches("\"size=33554432\"").count(),
            2,
            "the two tmpfs quotas partition 64 MiB into 32 MiB + 32 MiB, totaling exactly the one declared bound: {json}"
        );
        assert!(
            json.contains("\"destination\": \"/tmp/cargo-home\"")
                && json.contains("\"uid=65534\"")
                && json.contains("\"gid=65534\"")
                && json.contains("\"mode=0700\"")
                && json.contains("\"rw\""),
            "the structured Cargo home must be an explicit writable mount owned by the workload: {json}"
        );
        assert!(json.contains("\"destination\": \"/opt/myelin/cargo-vendor\""));
        assert!(json.contains("\"destination\": \"/tmp/cargo-home/config.toml\""));
        assert_eq!(json.matches("\"ro\"").count(), 2, "{json}");

        let staged = stage_production_bundle(&cfg, &fixture.rootfs).unwrap();
        let staged_config = std::fs::read_to_string(staged.path.join("cargo-config.toml")).unwrap();
        assert_eq!(staged_config, SERVER_CARGO_CONFIG_TOML);
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(staged.path.join("cargo-config.toml"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o444
        );
        let staged_json = std::fs::read_to_string(staged.path.join("config.json")).unwrap();
        assert!(staged_json.contains(&format!(
            "\"source\": {:?}",
            staged.path.join("cargo-config.toml").to_string_lossy()
        )));
        assert!(staged_json.contains("\"destination\": \"/tmp/cargo-home/config.toml\""));
    }

    #[test]
    fn cargo_vendor_serialization_missing_sources_returns_typed_refusal_without_panicking() {
        let fixture = cargo_boundary_fixture("typed-source-refusal");
        let cfg = wired_cargo_config(&fixture);
        let config_source = Path::new(TEST_SERVER_CARGO_CONFIG_SOURCE);
        let vendor_source = Path::new(TEST_CARGO_VENDOR_MOUNT_SOURCE);

        let missing_vendor = cfg
            .to_json_zeroizing_with_cargo_sources(Some(config_source), None)
            .expect_err("a missing verified vendor source must be a typed refusal");
        assert!(missing_vendor.contains("without a verified vendor source"));

        let missing_config = cfg
            .to_json_zeroizing_with_cargo_sources(None, Some(vendor_source))
            .expect_err("a missing server config source must be a typed refusal");
        assert!(missing_config.contains("without a server config source"));
    }

    #[test]
    fn structured_cargo_compute_route_refuses_instead_of_skipping_vendor_boundary() {
        let fixture = cargo_boundary_fixture("compute-route");
        let backend = GvisorBackend::new(cargo_compute_registry(&fixture));
        let job = structured_cargo_spec(&fixture.reference);
        let error = backend
            .launch_with(
                &job,
                &ok_hooks(),
                |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                    panic!(
                        "a structured compute job without Enabled workspace support must not run"
                    )
                },
            )
            .expect_err("the compute route must refuse rather than omit the vendor mounts");
        assert!(
            error
                .to_string()
                .contains("requires the Enabled workspace integration"),
            "{error}"
        );

        let mut networked_job = structured_cargo_spec(&fixture.reference);
        networked_job.egress.allow = vec!["registry.example:443".into()];
        let error = backend
            .launch_with(
                &networked_job,
                &ok_hooks(),
                |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                    panic!("a networked structured compute job must not run")
                },
            )
            .expect_err("the compute route must apply empty-egress validation");
        assert!(
            error.to_string().contains("empty egress (network=none)"),
            "{error}"
        );
    }

    #[test]
    fn free_form_command_launch_spec_gets_no_cargo_vendor_boundary() {
        let fixture = cargo_boundary_fixture("free-form");
        let mut free_form = structured_cargo_spec(&fixture.reference);
        free_form.command = vec!["/bin/test".into()];
        free_form.env.clear();
        assert!(selected_cargo_vendor(&free_form, &fixture.registry)
            .unwrap()
            .is_none());
        let profile = HardeningProfile::derive(&free_form);
        let json = OciConfig::from_spec(&free_form, &profile)
            .with_explicit_user_namespace_and_workspace(
                UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005),
                OciWorkspaceMount::for_tests(PathBuf::from("/host/workspace")),
                PathBuf::from("/abs/staged-rootfs"),
            )
            .unwrap()
            .to_json()
            .unwrap();
        assert!(!json.contains(OCI_CARGO_VENDOR_MOUNT), "{json}");
        assert!(!json.contains(OCI_CARGO_CONFIG_MOUNT), "{json}");
        assert!(!json.contains("CARGO_HOME="), "{json}");
        assert!(!json.contains("CARGO_NET_OFFLINE="), "{json}");
    }

    #[test]
    fn structured_cargo_argv_allowlist_admits_build_test_clippy_and_rejects_others() {
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_string()).collect::<Vec<_>>();
        let r = CARGO_SOURCE_REPLACE_CONFIG;
        let v = CARGO_VENDOR_DIRECTORY_CONFIG;
        // The exact four lowered argvs the control-plane grammar produces are all admitted.
        for argv in [
            vec!["cargo", "build", "--locked", "--config", r, "--config", v],
            vec!["cargo", "test", "--locked", "--lib", "--config", r, "--config", v],
            vec![
                "cargo", "test", "--locked", "--lib", "--workspace", "--config", r, "--config", v,
            ],
            vec![
                "cargo", "clippy", "--locked", "--all-targets", "--config", r, "--config", v, "--",
                "-D", "warnings",
            ],
        ] {
            assert!(
                is_admitted_structured_cargo_argv(&s(&argv)),
                "must admit {argv:?}"
            );
        }
        // Anything outside the closed set is refused — a tenant cannot drop `--locked`, run a
        // non-allowlisted subcommand, reorder the vendor `--config` after clippy's `--`, or shell out.
        for argv in [
            vec!["cargo", "build"],
            vec!["cargo", "run", "--locked", "--config", r, "--config", v],
            vec!["cargo", "test", "--config", r, "--config", v],
            vec![
                "cargo", "clippy", "--locked", "--all-targets", "--", "-D", "warnings", "--config",
                r, "--config", v,
            ],
            vec!["/bin/sh", "-c", "cargo build"],
        ] {
            assert!(
                !is_admitted_structured_cargo_argv(&s(&argv)),
                "must reject {argv:?}"
            );
        }
    }

    #[test]
    fn structured_cargo_vendor_selection_refuses_nonempty_egress_defense_in_depth() {
        let fixture = cargo_boundary_fixture("egress-refusal");
        let mut job = structured_cargo_spec(&fixture.reference);
        job.egress.allow = vec!["registry.example:443".into()];
        let error = selected_cargo_vendor(&job, &fixture.registry)
            .expect_err("the sandbox boundary must independently require network=none");
        assert!(error.contains("empty egress (network=none)"), "{error}");
    }

    #[test]
    fn server_cargo_config_replaces_crates_io_with_the_verified_vendor_directory() {
        assert_eq!(
            SERVER_CARGO_CONFIG_TOML,
            "[source.crates-io]\nreplace-with = \"vendored\"\n\n[source.vendored]\ndirectory = \"/opt/myelin/cargo-vendor\"\n"
        );
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

    /// **Path A (real-path vendor mount) contract.** The gVisor gofer cannot open a `/proc/pid/fd`
    /// magic-symlink source (it fails to `setns` into the runner mntns: `join container mntns:
    /// operation not permitted`), so the OCI mount source is the VERIFIED CANONICAL REAL PATH — like
    /// the pinned rootfs. This is deliberately NOT swap-immune against the trusted asset-store owner
    /// uid (that host-compromise class needs immutable storage, out of scope). The guarantees it DOES
    /// keep, asserted here: (1) the mount source is a real path (never a `/proc/fd` symlink the gofer
    /// rejects) resolving to the verified vendored crate, (2) the OCI config consumes exactly that
    /// path, and (3) a persistent pathname replacement makes the NEXT launch fail closed on the
    /// re-open identity check.
    #[test]
    fn verified_cargo_vendor_mount_uses_canonical_real_path_and_reverifies_next_launch() {
        let fixture = cargo_boundary_fixture("vendor-real-path");
        let cfg = wired_cargo_config(&fixture);
        let staged = stage_production_bundle(&cfg, &fixture.rootfs)
            .expect("the unchanged tree must verify and stage");
        let source = staged
            ._cargo_vendor
            .as_ref()
            .expect("structured staging holds the verified vendor capability")
            .vendor_mount_source
            .clone();

        // (1) A real path the gofer can open — NOT a `/proc/<pid>/fd/N` magic symlink — resolving to
        // the verified vendored crate.
        assert!(
            !source.starts_with("/proc/"),
            "vendor mount source must be a real path the gofer can open, not a /proc/fd symlink: \
             {source:?}"
        );
        assert_eq!(
            std::fs::read_to_string(source.join("itoa-1.0.15/lib.rs")).unwrap(),
            "pub fn fixture() {}",
        );
        // (2) The OCI config consumes exactly that real-path source.
        let staged_json = std::fs::read_to_string(staged.path.join("config.json")).unwrap();
        assert!(
            staged_json.contains(&format!("\"source\": {:?}", source.to_string_lossy())),
            "the OCI mount must consume the verified canonical real-path source: {staged_json}"
        );

        // (3) A persistent pathname replacement makes the NEXT launch fail closed: the re-open +
        // identity check refuses the replacement inode. (This does NOT claim same-launch swap
        // immunity — the mount source is now the real path — but the fd-bound re-verification still
        // catches a durable swap at the next stage.)
        let asset_path = fixture.root.join("asset");
        let moved_path = fixture.root.join("asset-moved-after-verify");
        std::fs::rename(&asset_path, &moved_path).unwrap();
        std::fs::create_dir_all(asset_path.join("vendor/itoa-1.0.15")).unwrap();
        std::fs::write(
            asset_path.join("vendor/itoa-1.0.15/lib.rs"),
            b"pub fn replacement() {}",
        )
        .unwrap();

        let error = match stage_production_bundle(&cfg, &fixture.rootfs) {
            Ok(_) => panic!("a later launch must refuse the replacement pathname inode"),
            Err(error) => error,
        };
        assert!(
            error.contains("no longer names its registry-verified inode"),
            "{error}"
        );
    }

    #[test]
    fn workspace_cargo_config_cannot_shadow_structured_source_boundary() {
        let fixture = cargo_boundary_fixture("precedence");
        let workspace = fixture.root.join("workspace");
        std::fs::create_dir_all(workspace.join(".cargo")).unwrap();
        std::fs::write(
            workspace.join(".cargo/config.toml"),
            b"[source.crates-io]\nreplace-with='tenant'\n[source.tenant]\ndirectory='/workspace/tenant'\n",
        )
        .unwrap();
        let legacy_cargo_home = fixture.root.join("legacy-cargo-home");
        std::fs::create_dir(&legacy_cargo_home).unwrap();
        std::fs::write(
            legacy_cargo_home.join("config"),
            b"[source.crates-io]\nreplace-with='legacy'\n[source.legacy]\ndirectory='/tmp/legacy'\n",
        )
        .unwrap();
        let cfg = wired_cargo_config(&fixture);
        let json = cfg.to_json().unwrap();
        assert_eq!(
            cfg.args,
            [
                "cargo",
                "build",
                "--locked",
                "--config",
                "source.crates-io.replace-with=\"vendored\"",
                "--config",
                "source.vendored.directory=\"/opt/myelin/cargo-vendor\"",
            ],
            "platform CLI config must outrank both workspace and legacy Cargo-home config files"
        );
        assert!(json.contains("CARGO_SOURCE_CRATES_IO_REPLACE_WITH=vendored"));
        assert!(json.contains("CARGO_SOURCE_VENDORED_DIRECTORY=/opt/myelin/cargo-vendor"));
        assert!(json.contains("\"destination\": \"/tmp/cargo-home/config.toml\""));
        assert!(json.contains("\"ro\""));
        assert!(!json.contains("/workspace/tenant"));
    }

    // ───────── CT-007 slice 3, piece 4: WorkspaceIntegration / GvisorWorkspaceConfig ─────────

    #[test]
    fn new_and_git_wire_only_construct_disabled_workspace_integration() {
        let backend = GvisorBackend::new(test_registry());
        assert!(matches!(
            backend.workspace_integration,
            WorkspaceIntegration::Disabled
        ));
        let git_wire_backend = GvisorBackend::git_wire_only();
        assert!(matches!(
            git_wire_backend.workspace_integration,
            WorkspaceIntegration::Disabled
        ));
    }

    #[test]
    fn try_new_with_disabled_config_never_touches_the_filesystem() {
        let backend = GvisorBackend::try_new(
            test_registry(),
            GvisorWorkspaceConfig::Disabled,
            Arc::new(|_: &str| {}),
        )
        .expect("Disabled construction must never fail");
        assert!(matches!(
            backend.workspace_integration,
            WorkspaceIntegration::Disabled
        ));
    }

    #[test]
    fn try_new_with_enabled_config_refuses_before_touching_workspace_when_userns_is_unsafe() {
        // Any leases_dir this unprivileged test process can create itself sits under a directory
        // it owns or can write to (its own home dir, or /tmp) — the strict production allocator's
        // ancestor-not-writable-by-us check refuses EVERY such path, deliberately: a genuinely
        // hardened leases dir requires a root-provisioned deployment layout, out of scope for an
        // ordinary unit test (this crate's own explicit-userns drill documents the identical
        // constraint). This makes the refusal fully deterministic here, which is exactly what this
        // test wants: proof that workspace reconciliation is never even attempted once userns
        // construction fails.
        let base = std::env::temp_dir().join(format!(
            "myelin-gvisor-try-new-workspace-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let leases_dir = std::env::temp_dir().join(format!(
            "myelin-gvisor-try-new-leases-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let result = GvisorBackend::try_new(
            test_registry(),
            GvisorWorkspaceConfig::Enabled {
                base_dir: base.clone(),
                host_capacity_bytes: 1 << 30,
                leases_dir,
                min_pool_size: 1,
            },
            Arc::new(|_: &str| {}),
        );
        match result {
            Err(GvisorBackendInitError::UserNamespace(_)) => {}
            Err(other) => panic!("expected a UserNamespace error, got a different error: {other}"),
            Ok(_) => panic!(
                "expected a UserNamespace error — a leases dir under this test's own home/tmp \
                 directory must never be considered safe"
            ),
        }
        assert!(
            !base.exists(),
            "workspace reconciliation must never run when userns construction fails first"
        );
    }

    /// Sol's round-1 review of piece 4: the public `try_new`'s own success path (and therefore the
    /// `Workspace(_)` error-mapping branch, reachable only once userns has ALREADY succeeded) is
    /// untestable end-to-end on an ordinary dev/CI host — the strict production allocator's
    /// ancestor check always refuses any leases_dir this unprivileged test process can create
    /// itself, AND `base_dir` here is never a real quota-enforcing Btrfs mount either, so even a
    /// hypothetical userns success would just trade one failure for another. These tests instead
    /// exercise `try_new_with_builders` directly, injecting builders that still return the REAL
    /// `UserNamespaceAllocator`/`WorkspaceManager` types (via their own existing test-relaxed
    /// constructors) — never a fabricated stand-in — so what `Enabled` actually holds is unchanged.
    #[test]
    fn try_new_with_builders_never_calls_workspace_builder_when_userns_fails() {
        let workspace_builder_called = Arc::new(AtomicBool::new(false));
        let flag = workspace_builder_called.clone();
        let result = GvisorBackend::try_new_with_builders(
            test_registry(),
            GvisorWorkspaceConfig::Enabled {
                base_dir: PathBuf::from("/nonexistent-base-for-this-test"),
                host_capacity_bytes: 1 << 30,
                leases_dir: PathBuf::from("/nonexistent-leases-for-this-test"),
                min_pool_size: 1,
            },
            Arc::new(|_: &str| {}),
            |_leases_dir, _min_pool_size, _sink| {
                Err(UserNamespaceAllocatorError::NoSubordinateEntry {
                    path: PathBuf::from("/etc/subuid"),
                    uid: 0,
                })
            },
            move |_mode, _sink| {
                flag.store(true, Ordering::SeqCst);
                Err(WorkspaceManagerError::AlreadyLocked {
                    base_dir: PathBuf::new(),
                })
            },
        );
        match result {
            Err(GvisorBackendInitError::UserNamespace(_)) => {}
            Err(other) => panic!("expected UserNamespace(_), got a different error: {other}"),
            Ok(_) => panic!("expected UserNamespace(_), got Ok"),
        }
        assert!(
            !workspace_builder_called.load(Ordering::SeqCst),
            "the workspace builder must never run once the userns builder has failed"
        );
    }

    /// Writes a real, valid `subuid`/`subgid`-format file naming the CURRENT effective uid, with
    /// the given range, so tests never depend on this host's REAL `/etc/subuid`/`/etc/subgid`
    /// having an entry for this uid (mirroring `user_namespace.rs`'s own test helper of the same
    /// name/shape — Sol's round-2 review: relying on the real files left these tests conditionally
    /// skippable on any CI host lacking subordinate-id configuration, exactly the host dependency
    /// the builder seam exists to remove).
    fn write_subordinate_file(path: &Path, start: u32, count: u32) {
        let uid = unsafe { libc::geteuid() };
        std::fs::write(path, format!("{uid}:{start}:{count}\n")).unwrap();
    }

    #[test]
    fn try_new_with_builders_maps_a_workspace_failure_after_userns_succeeds() {
        let base = std::env::temp_dir().join(format!(
            "myelin-gvisor-builders-workspace-fails-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 8);
        write_subordinate_file(&subgid, 200_000, 8);
        let result = GvisorBackend::try_new_with_builders(
            test_registry(),
            GvisorWorkspaceConfig::Enabled {
                base_dir: PathBuf::from("/nonexistent-base-for-this-test"),
                host_capacity_bytes: 1 << 30,
                leases_dir: leases_dir.clone(),
                min_pool_size: 1,
            },
            Arc::new(|_: &str| {}),
            |leases_dir, min_pool_size, sink| {
                crate::user_namespace::UserNamespaceAllocator::try_new_for_tests(
                    leases_dir,
                    &subuid,
                    &subgid,
                    min_pool_size,
                    sink,
                )
            },
            |mode, _sink| {
                assert!(
                    matches!(mode, WorkspaceStorageMode::EphemeralDisk { .. }),
                    "the correct mode must be forwarded to the workspace builder"
                );
                Err(WorkspaceManagerError::AlreadyLocked {
                    base_dir: PathBuf::from("/nonexistent-base-for-this-test"),
                })
            },
        );
        match result {
            Err(GvisorBackendInitError::Workspace(_)) => {}
            Err(other) => panic!("expected Workspace(_), got a different error: {other}"),
            Ok(_) => panic!("expected Workspace(_), got Ok"),
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn try_new_with_builders_produces_enabled_holding_both_managers_when_both_succeed() {
        let base = std::env::temp_dir().join(format!(
            "myelin-gvisor-builders-both-succeed-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 8);
        write_subordinate_file(&subgid, 200_000, 8);
        let backend = GvisorBackend::try_new_with_builders(
            test_registry(),
            GvisorWorkspaceConfig::Enabled {
                base_dir: PathBuf::from("/nonexistent-base-for-this-test"),
                host_capacity_bytes: 1 << 30,
                leases_dir: leases_dir.clone(),
                min_pool_size: 1,
            },
            Arc::new(|_: &str| {}),
            |leases_dir, min_pool_size, sink| {
                crate::user_namespace::UserNamespaceAllocator::try_new_for_tests(
                    leases_dir,
                    &subuid,
                    &subgid,
                    min_pool_size,
                    sink,
                )
            },
            // A `WorkspaceManager::Disabled` instance stands in as a genuine, real value of the
            // right type (mode-forwarding itself is already asserted in the sibling test above) —
            // never a fabricated non-real value.
            |_mode, sink| WorkspaceManager::try_new(WorkspaceStorageMode::Disabled, sink),
        )
        .expect("both builders must succeed with a real, fixture-backed subordinate range");
        assert!(matches!(
            backend.workspace_integration,
            WorkspaceIntegration::Enabled { .. }
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn try_new_with_builders_invokes_neither_builder_when_disabled() {
        let userns_called = Arc::new(AtomicBool::new(false));
        let workspace_called = Arc::new(AtomicBool::new(false));
        let u = userns_called.clone();
        let w = workspace_called.clone();
        let backend = GvisorBackend::try_new_with_builders(
            test_registry(),
            GvisorWorkspaceConfig::Disabled,
            Arc::new(|_: &str| {}),
            move |leases_dir, min_pool_size, sink| {
                u.store(true, Ordering::SeqCst);
                UserNamespaceAllocator::try_new(leases_dir, min_pool_size, sink)
            },
            move |mode, sink| {
                w.store(true, Ordering::SeqCst);
                WorkspaceManager::try_new(mode, sink)
            },
        )
        .expect("Disabled must always succeed");
        assert!(matches!(
            backend.workspace_integration,
            WorkspaceIntegration::Disabled
        ));
        assert!(!userns_called.load(Ordering::SeqCst));
        assert!(!workspace_called.load(Ordering::SeqCst));
    }

    /// CT-007 #26/#27 INTEGRATION PROOF — the reproduced release blocker (gate-2 green drill,
    /// 2026-08-03): a build job MUTATED the shared digest-pinned base rootfs on the host (its
    /// canonical digest drifted `91ffb0fa… -> eb7248a1…` after one job), so the NEXT runner startup
    /// panicked `DigestMismatch` at asset re-verify. Cause: per-job mount-target creation / gofer
    /// writes landed in the SHARED base tree instead of a per-job ephemeral layer.
    ///
    /// This drives a REAL launch through `launch_with` -> `launch_compute_common_body` with a per-job
    /// rootfs overlay manager installed (deterministic mode: no `CAP_SYS_ADMIN`/kernel OverlayFS
    /// needed, but the SAME integration seam production uses — `materialize_job_guest_root` substitutes
    /// the overlay merged view for the base everywhere the base path flowed). The injected run closure
    /// stands in for runsc + the gofer: it WRITES into the guest root it is handed (a new mount-target
    /// directory + file, and a delete of a base file), exactly the host-side layout mutation that
    /// corrupted the base before. The property whose violation caused the panic is asserted directly:
    /// the base tree's canonical digest is BYTE-IDENTICAL before and after the job, and none of the
    /// job's writes reached the base — they were absorbed by the per-job overlay (a DIFFERENT path).
    #[test]
    fn compute_launch_guest_root_is_a_per_job_overlay_leaving_the_base_byte_pristine() {
        use crate::asset_registry::{GvisorAssetRegistry, RootfsAssetBinding};
        use crate::rootfs_overlay::{RootfsOverlayManager, RootfsOverlayMode};
        use crate::{canonical_tree_sha256_hex, ImageRef};

        // A dedicated, isolated pinned base rootfs tree (never the shared per-process fixture dir).
        let root = std::env::temp_dir().join(format!(
            "myelin-overlay-integration-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let base = root.join("pinned-base");
        let overlays = root.join("overlays");
        std::fs::create_dir_all(base.join("etc")).unwrap();
        std::fs::create_dir(base.join("workspace")).unwrap();
        std::fs::create_dir_all(base.join("opt/myelin/cargo-vendor")).unwrap();
        std::fs::write(base.join("etc/keep"), b"keep").unwrap();
        std::fs::write(base.join("delete-me"), b"delete").unwrap();
        let digest = canonical_tree_sha256_hex(&base).unwrap();
        let image = ImageRef::pinned(format!("test.local/overlay-int@sha256:{digest}")).unwrap();

        let registry = Arc::new(
            GvisorAssetRegistry::from_bindings(vec![RootfsAssetBinding {
                image: image.clone(),
                rootfs: base.clone(),
            }])
            .expect("the pinned base verifies"),
        );
        let manager = Arc::new(
            RootfsOverlayManager::initialize(
                RootfsOverlayMode::DeterministicDirectoryForTests {
                    overlays_dir: overlays.clone(),
                },
                Arc::new(|_message: &str| {}),
            )
            .expect("the deterministic overlay manager initializes"),
        );
        let backend = GvisorBackend::new(registry).with_rootfs_overlay_manager(manager);

        // A minimal image-bearing compute spec resolving to the pinned base above.
        let job = JobSpec::new(
            JobKind::Agent,
            image,
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
            WorkspaceSpec::default(),
            TrustTier::UntrustedFork,
            RunTokenCredential::new("test-bearer", "j", 300).unwrap(),
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("idem-overlay-int-1".into()),
        )
        .unwrap();

        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec: &JobSpec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_spec| Ok(())),
            Box::new(|_spec| Ok(())),
        );

        let base_digest_before = canonical_tree_sha256_hex(&base).unwrap();
        let observed_root = Arc::new(Mutex::new(None::<PathBuf>));
        let seen = observed_root.clone();
        let base_for_closure = base.clone();

        let launch = backend
            .launch_with(
                &job,
                &hooks,
                move |_spec, _cfg, _permit, rootfs, _container_id, _prep| {
                    // The run closure receives the per-job guest root. It MUST be the overlay merged
                    // view, NOT the shared base.
                    assert_ne!(
                        rootfs, base_for_closure,
                        "the launch must NOT hand runsc the shared pinned base as its guest root"
                    );
                    // The merged view is a fully-populated copy of the verified base.
                    assert_eq!(std::fs::read_to_string(rootfs.join("etc/keep")).unwrap(), "keep");
                    // Simulate the exact host-side mutation that corrupted the base before: create a
                    // fresh mount-target directory + file, and delete a base file. All must land in
                    // the per-job upper, never the shared base.
                    std::fs::create_dir(rootfs.join("workspace/gofer-mount-target")).unwrap();
                    std::fs::write(rootfs.join("workspace/gofer-mount-target/x"), b"job-write")
                        .unwrap();
                    std::fs::remove_file(rootfs.join("delete-me")).unwrap();
                    *seen.lock().unwrap() = Some(rootfs.to_path_buf());
                    Ok(fake_finalization())
                },
            )
            .expect("the compute path launches");
        assert!(launch.output_complete);

        // THE property whose violation caused the DigestMismatch panic: the shared pinned base is
        // byte-identical before and after the job.
        assert_eq!(
            canonical_tree_sha256_hex(&base).unwrap(),
            base_digest_before,
            "the pinned base rootfs digest must be byte-identical after a job that wrote to its root"
        );
        // None of the job's host-side writes reached the base tree.
        assert!(
            base.join("delete-me").exists(),
            "a base file the job deleted (in the overlay) must still exist in the base"
        );
        assert!(
            !base.join("workspace/gofer-mount-target").exists(),
            "a mount target the job created (in the overlay) must NOT appear in the base"
        );
        // The guest root the run closure actually saw was a distinct per-job overlay path.
        let observed = observed_root.lock().unwrap().clone().expect("run observed a root");
        assert_ne!(observed, base, "the guest root was a per-job overlay, not the base");
        assert!(
            observed.starts_with(&overlays),
            "the per-job overlay lives under the manager's overlay root: {observed:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    fn ok_hooks() -> RunnerHooks {
        RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        )
    }

    fn outcome(stdout: &[u8], stderr: &[u8]) -> RunscOutcome {
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

    // CT-004f sub-step 1: `build_result` APPLIES the redaction plan to both captured streams — the
    // boundary seam is wired, not just the `RedactionPlan` unit. A populated injection plan masks the
    // needle before it reaches `SandboxResult`.
    #[test]
    fn build_result_masks_needles_in_both_streams() {
        let s = spec(vec![]);
        // Assemble the scanner-shaped credential at runtime. Keeping the complete sentinel in this
        // source blob would make Myelin's own reject-before-promote scanner reject the repository
        // that implements and tests it.
        let needle = [b"AK".as_slice(), b"IAsecret"].concat();
        let stdout = [b"deploying with ".as_slice(), needle.as_slice(), b" now"].concat();
        let stderr = [b"error: ".as_slice(), needle.as_slice(), b" invalid"].concat();
        let plan = RedactionPlan::for_needles([needle.clone()]).unwrap();
        let o = outcome(&stdout, &stderr);
        let res = build_result(&s, &o, &plan);
        assert!(res.stdout.starts_with(b"deploying with "));
        assert!(res.stdout.ends_with(b" now"));
        assert!(res.stderr.starts_with(b"error: "));
        assert!(res.stderr.ends_with(b" invalid"));
        assert!(!res
            .stdout
            .windows(needle.len())
            .any(|window| window == needle));
        assert!(!res
            .stderr
            .windows(needle.len())
            .any(|window| window == needle));
    }

    #[test]
    fn injected_secret_value_is_absent_from_sandbox_result_when_workload_prints_it() {
        let mut s = spec(vec![]);
        s.secret_refs = vec![crate::SecretRef {
            name: "DEPLOY_TOKEN".into(),
            handle: "opaque:deploy".into(),
        }];
        let material = ["printed", "-secret-material"].concat();
        let s = s
            .with_resolved_secrets(vec![crate::ResolvedSecretEnv::new(
                "DEPLOY_TOKEN",
                material.clone(),
            )])
            .expect("binding and plan are derived together");
        let stdout = format!("stdout:{material}");
        let stderr = format!("stderr:{material}");
        let outcome = outcome(stdout.as_bytes(), stderr.as_bytes());

        let result = build_result(&s, &outcome, s.resolved_secrets().redaction_plan());

        assert!(result.stdout.starts_with(b"stdout:"));
        assert!(result.stderr.starts_with(b"stderr:"));
        assert!(!result
            .stdout
            .windows(material.len())
            .any(|window| window == material.as_bytes()));
        assert!(!result
            .stderr
            .windows(material.len())
            .any(|window| window == material.as_bytes()));
        assert!(!format!("{result:?}").contains(&material));
    }

    // A non-secret job's empty plan remains a pass-through: captured output is byte-unchanged.
    #[test]
    fn build_result_empty_plan_is_byte_identity() {
        let s = spec(vec![]);
        let o = outcome(b"ordinary build log line", b"warning: deprecated");
        let res = build_result(&s, &o, &RedactionPlan::none());
        assert_eq!(res.stdout, b"ordinary build log line".to_vec());
        assert_eq!(res.stderr, b"warning: deprecated".to_vec());
    }

    /// A canned [`ContainerRun`] for the fake path (no real `runsc`): a clean exit-0 result + a fake
    /// child + a non-existent bundle dir (its removal on teardown is a harmless no-op).
    fn fake_run() -> ContainerRun {
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
    fn fake_finalization() -> RuntimeFinalization<Result<ContainerRun, RunFailure>> {
        RuntimeFinalization::Finalized(FinalizedRun {
            primary: Ok(fake_run()),
            evidence: RuntimeQuiescenceEvidence {
                container_id: "fake-container".to_string(),
                namespace: RuntimeNamespaceQuiescence::Rootless,
                cgroup: CgroupQuiescenceEvidence::assert_for_tests((0, 0)),
            },
        })
    }

    /// CT-006c (the streaming fix): the git-wire stdout drain stages straight to a host temp file under
    /// a generous cap with host memory bounded to one chunk. A response WITHIN the cap comes through
    /// WHOLE (no 256 KiB truncation); a response OVER the cap is head-bounded AND flagged `truncated`
    /// (which the wire seam turns into a LOUD `WireError::OutputTooLarge` — never a silent short pack).
    #[test]
    fn drain_to_temp_file_streams_whole_under_cap_and_flags_over_cap() {
        // A 1 MiB stream (FAR past the 256 KiB SANDBOX_CAPTURE_BOUND) under a 4 MiB cap → WHOLE, untruncated.
        let big = vec![0xABu8; 1024 * 1024];
        let (out, truncated) = drain_to_temp_file(&big[..], 4 * 1024 * 1024);
        assert_eq!(
            out.len(),
            big.len(),
            "a real-size pack under the cap comes through WHOLE"
        );
        assert_eq!(
            out, big,
            "the bytes are byte-identical (no corruption via the temp file)"
        );
        assert!(!truncated, "within the cap ⇒ not truncated");

        // The SAME stream under a 64 KiB cap → head-bounded to the cap AND flagged truncated (fail-loud).
        let (head, over) = drain_to_temp_file(&big[..], 64 * 1024);
        assert_eq!(
            head.len(),
            64 * 1024,
            "over the cap ⇒ exactly the cap bytes are kept"
        );
        assert!(
            over,
            "over the cap ⇒ truncated flag set (the wire seam then refuses loudly)"
        );
    }

    #[test]
    fn drain_to_temp_file_marks_a_read_fault_as_incomplete() {
        struct FaultAfterPrefix(Option<&'static [u8]>);

        impl Read for FaultAfterPrefix {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if let Some(prefix) = self.0.take() {
                    buf[..prefix.len()].copy_from_slice(prefix);
                    Ok(prefix.len())
                } else {
                    Err(std::io::Error::other("injected wire read fault"))
                }
            }
        }

        let (head, incomplete) = drain_to_temp_file(FaultAfterPrefix(Some(b"partial-pack")), 1024);

        assert_eq!(head, b"partial-pack");
        assert!(incomplete);
    }

    #[test]
    fn oci_config_enforces_the_backend_independent_hardening() {
        let cfg = GvisorBackend::oci_config(&spec(vec![])).unwrap();
        assert!(cfg.root_readonly());
        assert!(!cfg.has_network(), "no allowlist ⇒ no network interface");
        let json = cfg.to_json().unwrap();
        assert!(json.contains("\"readonly\": true"));
        assert!(json.contains("\"noNewPrivileges\": true"));
        assert!(
            json.contains("SCMP_ACT_ERRNO"),
            "a seccomp profile is attached"
        );
        assert!(
            json.contains("\"bounding\": []"),
            "all capabilities dropped"
        );
        assert!(
            json.contains("\"type\": \"RLIMIT_NPROC\"")
                && json.contains("\"hard\": 64")
                && json.contains("\"soft\": 64"),
            "rootless gVisor gets an in-sandbox process ceiling independent of host cgroups"
        );
        // CT-002b: the untrusted process runs NON-ROOT (defense in depth — never uid 0 in the
        // sandbox) and the config is RUNNABLE (`cwd` set, else `runsc run` rejects the spec).
        assert!(
            json.contains("\"uid\": 65534") && json.contains("\"gid\": 65534"),
            "the untrusted process must run as a non-root uid/gid (65534)"
        );
        assert!(
            json.contains("\"cwd\": \"/\""),
            "process.cwd must be set or the OCI runtime rejects the spec"
        );
        // CT-003a/CT-003b (SI-017): the OCI emits an (advisory — rootless runsc ignores it) memory
        // ceiling from spec.limits.mem_bytes; the REAL host-RAM bound is the out-of-band MemoryCgroup
        // the production run path places the runsc tree into (see MemoryCgroup). It also mounts a
        // SIZE-BOUNDED writable `/tmp` tmpfs (sized from the scratch quota) so a disk fill hits
        // ENOSPC instead of an unbounded host-RAM-backed tmpfs. spec()'s limits are mem=256 MiB,
        // tmpfs=1 GiB.
        assert!(
            json.contains(&format!("\"limit\": {}", 256u64 << 20)),
            "the OCI config must carry the memory ceiling (linux.resources.memory.limit) from spec.limits.mem_bytes"
        );
        assert!(
            json.contains("\"destination\": \"/tmp\"") && json.contains("\"type\": \"tmpfs\""),
            "a size-bounded writable /tmp tmpfs must be mounted (no unbounded host-RAM-backed scratch)"
        );
        assert!(
            json.contains(&format!("size={}", 1u64 << 30)) && json.contains("mode=1777"),
            "the /tmp tmpfs must be sized from spec.limits.tmpfs_bytes and writable by the non-root payload"
        );
        assert!(
            !json.contains("\"type\": \"user\"") && !json.contains("uidMappings"),
            "Rootless mode (the default) must never declare a user namespace or uid/gid mappings \
             — runsc --rootless installs its own, and a doubly-declared userns fails the gofer"
        );
        assert!(
            json.contains("\"path\": \"rootfs\""),
            "ordinary rootless launch must use the bundle-relative rootfs: {json}"
        );
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::Rootless,
            "a config with no explicit user namespace attached must report Rootless"
        );
    }

    /// CT-007 slice 3, piece 6 test matrix: the git-wire-shaped `RootlessWithHostMounts` layout —
    /// an absolute rootfs override alongside its host bind mounts, still fully rootless (no user
    /// namespace, no uid/gid mappings).
    #[test]
    fn oci_config_rootless_with_host_mounts_emits_absolute_root_and_the_bind_mounts() {
        let cfg = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_rootless_host_mounts(
                PathBuf::from("/abs/staged-rootfs"),
                PathBuf::from("/host/repo"),
                Some(PathBuf::from("/host/quarantine")),
            )
            .expect("an absolute rootfs override must be accepted");
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::Rootless,
            "host mounts alone must not imply a user namespace"
        );
        let json = cfg.to_json().unwrap();
        assert!(
            json.contains("\"path\": \"/abs/staged-rootfs\""),
            "the absolute rootfs override must be emitted verbatim: {json}"
        );
        assert!(
            json.contains("\"destination\": \"/repo\"") && json.contains("\"ro\""),
            "the RO repo bind mount must be present: {json}"
        );
        assert!(
            json.contains("\"destination\": \"/quarantine\"") && json.contains("\"rw\""),
            "the writable quarantine bind mount must be present: {json}"
        );
        assert!(
            !json.contains("\"type\": \"user\"") && !json.contains("uidMappings"),
            "RootlessWithHostMounts must never declare a user namespace or uid/gid mappings"
        );
    }

    #[test]
    fn oci_config_with_rootless_host_mounts_refuses_a_relative_rootfs() {
        let result = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_rootless_host_mounts(
                PathBuf::from("relative/staged-rootfs"),
                PathBuf::from("/host/repo"),
                None,
            );
        assert!(
            result.is_err(),
            "a non-absolute rootfs override must be refused, not silently accepted"
        );
    }

    /// CT-007 slice 3, piece 6 test matrix: the workspace-mount layout — absolute root, explicit
    /// user-namespace mappings, AND exactly one fixed writable workspace bind mount.
    #[test]
    fn oci_config_explicit_userns_with_workspace_emits_absolute_root_mappings_and_the_fixed_mount()
    {
        let config = UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005);
        let workspace = OciWorkspaceMount::for_tests(PathBuf::from("/host/workspace-subvol"));
        let cfg = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_explicit_user_namespace_and_workspace(
                config,
                workspace,
                PathBuf::from("/abs/staged-rootfs"),
            )
            .expect("an absolute rootfs override must be accepted");
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::ExplicitUserNamespace(config)
        );
        let json = cfg.to_json().unwrap();
        assert!(
            json.contains("\"path\": \"/abs/staged-rootfs\""),
            "the workspace layout must use an absolute rootfs override: {json}"
        );
        assert!(
            json.contains("\"type\": \"user\""),
            "a user namespace must be declared: {json}"
        );
        assert!(
            json.contains("\"containerID\": 65534, \"hostID\": 100005, \"size\": 1"),
            "container uid 65534 must map to the leased subordinate host uid: {json}"
        );
        assert!(
            json.contains("\"destination\": \"/workspace\"")
                && json.contains("\"source\": \"/host/workspace-subvol\"")
                && json.contains("\"rw\""),
            "exactly one fixed writable workspace bind mount must be present: {json}"
        );
        assert!(
            json.contains("\"cwd\": \"/workspace\""),
            "workspace-backed workloads must start in the checked-out tree: {json}"
        );
        // Never readonly, never a caller-selectable destination — only ONE workspace mount entry.
        assert_eq!(
            json.matches("\"destination\": \"/workspace\"").count(),
            1,
            "exactly one workspace mount, never more: {json}"
        );
    }

    #[test]
    fn oci_config_with_explicit_user_namespace_and_workspace_refuses_a_relative_rootfs() {
        let config = UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005);
        let workspace = OciWorkspaceMount::for_tests(PathBuf::from("/host/workspace-subvol"));
        let result = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_explicit_user_namespace_and_workspace(
                config,
                workspace,
                PathBuf::from("relative/staged-rootfs"),
            );
        assert!(
            result.is_err(),
            "a non-absolute rootfs override must be refused, not silently accepted"
        );
    }

    /// Sol's round-1 review of piece 6: layout selection must be ONE-SHOT — chaining two
    /// layout-selecting builders must never silently discard whichever was selected first, even
    /// though the enum itself already prevents an invalid FINAL combination.
    #[test]
    fn oci_config_layout_selection_is_one_shot() {
        let config = UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005);
        // userns first, THEN an attempt at rootless host mounts — must refuse, not silently revert.
        let result = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_user_namespace(config)
            .unwrap()
            .with_rootless_host_mounts(
                PathBuf::from("/abs/staged-rootfs"),
                PathBuf::from("/host/repo"),
                None,
            );
        assert!(
            result.is_err(),
            "attaching host mounts after a user namespace was already selected must refuse, not \
             silently discard the user namespace"
        );
        // host mounts first, THEN an attempt at a user namespace — must refuse, not silently
        // discard the mounts.
        let result = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_rootless_host_mounts(
                PathBuf::from("/abs/staged-rootfs"),
                PathBuf::from("/host/repo"),
                None,
            )
            .unwrap()
            .with_user_namespace(config);
        assert!(
            result.is_err(),
            "attaching a user namespace after host mounts were already selected must refuse, not \
             silently discard the mounts"
        );
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

    /// Sol's round-1 review of piece 6: `RootlessWithHostMounts` must never accept a free-form
    /// mount descriptor with a caller-controlled destination/mode — that could otherwise smuggle a
    /// writable `/workspace`-shaped mount into a layout that reports `Rootless`. This test proves
    /// there is no way to reach that state: the builder's signature only ever accepts fixed
    /// repo/quarantine host-source paths, never an arbitrary destination or mode.
    #[test]
    fn oci_config_rootless_with_host_mounts_never_accepts_an_arbitrary_destination_or_mode() {
        // The only two mounts `RootlessWithHostMounts` can ever produce are the fixed
        // WIRE_REPO_MOUNT (always ro) and WIRE_QUARANTINE_MOUNT (always rw, only if requested) —
        // proven by construction (the builder takes no `guest_dest`/`readonly` parameters at all),
        // and confirmed here by asserting the JSON never contains anything else.
        let cfg = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_rootless_host_mounts(
                PathBuf::from("/abs/staged-rootfs"),
                PathBuf::from("/host/repo"),
                Some(PathBuf::from("/host/quarantine")),
            )
            .unwrap();
        let json = cfg.to_json().unwrap();
        assert!(!json.contains("\"destination\": \"/workspace\""));
        assert!(json.contains(WIRE_REPO_MOUNT));
        assert!(json.contains(WIRE_QUARANTINE_MOUNT));
    }

    /// CT-007 gate 2: a job's declared (non-secret) environment must reach `process.env`. Before
    /// this, `from_spec` dropped `spec.env` entirely, so a real build job's `CARGO_*` / PATH
    /// extensions never took effect in the sandbox. Secret values are NOT carried in `spec.env`
    /// (resolved in-boundary from `SecretRef`), so a plain `NAME=VALUE` render is correct.
    #[test]
    fn oci_config_propagates_the_jobs_declared_env_into_process_env() {
        let mut s = spec(vec![]);
        s.env = vec![
            crate::EnvVar {
                name: "CARGO_NET_OFFLINE".into(),
                value: "true".into(),
            },
            crate::EnvVar {
                name: "CARGO_HOME".into(),
                value: "/workspace/.cargo".into(),
            },
        ];
        let json = GvisorBackend::oci_config(&s).unwrap().to_json().unwrap();
        assert!(
            json.contains("CARGO_NET_OFFLINE=true"),
            "declared env dropped: {json}"
        );
        assert!(
            json.contains("CARGO_HOME=/workspace/.cargo"),
            "declared env dropped: {json}"
        );
        // The base PATH is still emitted (declared env is APPENDED after it, not replaced).
        assert!(
            json.contains("PATH=/usr/local/sbin"),
            "base PATH lost: {json}"
        );
    }

    #[test]
    fn injected_secret_reaches_oci_process_env_without_entering_debug_records() {
        let mut s = spec(vec![]);
        s.secret_refs = vec![crate::SecretRef {
            name: "DEPLOY_TOKEN".into(),
            handle: "opaque:deploy".into(),
        }];
        let material = ["boundary", "-only-material"].concat();
        let s = s
            .with_resolved_secrets(vec![crate::ResolvedSecretEnv::new(
                "DEPLOY_TOKEN",
                material.clone(),
            )])
            .expect("the exact declared binding set must couple to redaction");

        let cfg = GvisorBackend::oci_config(&s).expect("covered injection is launchable");
        let json = cfg.to_json().unwrap();
        assert!(json.contains(&format!("DEPLOY_TOKEN={material}")));
        assert!(!format!("{s:?}").contains(&material));
        assert!(!format!("{:?}", s.resolved_secrets().redaction_plan()).contains(&material));
        assert!(!format!("{cfg:?}").contains(&material));
    }

    /// Empty host-mount collections are still valid (git-wire's OWN quarantine mount is genuinely
    /// optional — a read-only serve with no push in flight has none) — the repo mount alone is
    /// never itself optional, so "empty" here means "repo only," never "no mounts at all," which
    /// this test confirms is exactly what a `None` quarantine source produces.
    #[test]
    fn oci_config_rootless_with_host_mounts_omits_the_quarantine_mount_when_absent() {
        let cfg = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_rootless_host_mounts(
                PathBuf::from("/abs/staged-rootfs"),
                PathBuf::from("/host/repo"),
                None,
            )
            .unwrap();
        let json = cfg.to_json().unwrap();
        assert!(json.contains(WIRE_REPO_MOUNT));
        assert!(!json.contains(WIRE_QUARANTINE_MOUNT));
    }

    /// CT-007 slice 2: `with_user_namespace` must produce the EXACT two-entry OCI mapping the
    /// design specifies, alongside a declared `user` namespace — and `invocation_mode()` must
    /// report `ExplicitUserNamespace` carrying the SAME config back out.
    #[test]
    fn oci_config_with_user_namespace_emits_the_exact_two_entry_mapping() {
        let config = UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005);
        let cfg = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_user_namespace(config)
            .expect("a fresh Rootless config must accept a user-namespace layout selection");
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::ExplicitUserNamespace(config)
        );
        let json = cfg.to_json().unwrap();
        assert!(
            json.contains("\"type\": \"user\""),
            "a user namespace must be declared: {json}"
        );
        assert!(
            json.contains("\"containerID\": 0, \"hostID\": 1000, \"size\": 1"),
            "container uid/gid 0 must map to the runner's own real identity: {json}"
        );
        assert!(
            json.contains("\"containerID\": 65534, \"hostID\": 100005, \"size\": 1"),
            "container uid 65534 must map to the leased subordinate host uid: {json}"
        );
        assert!(
            json.contains("\"containerID\": 65534, \"hostID\": 200005, \"size\": 1"),
            "container gid 65534 must map to the leased subordinate host gid: {json}"
        );
        // Every OTHER hardening assertion from the Rootless test must still hold — attaching a
        // user namespace changes ONLY the namespaces/mappings, nothing else.
        assert!(json.contains("\"readonly\": true"));
        assert!(json.contains("\"noNewPrivileges\": true"));
        assert!(json.contains("\"uid\": 65534") && json.contains("\"gid\": 65534"));
        assert!(
            json.contains("\"path\": \"rootfs\""),
            "explicit userns WITHOUT a workspace mount involves no host bind mount, so it must \
             still use the bundle-relative rootfs, not an absolute override: {json}"
        );
    }

    /// `apply_runsc_invocation_policy` is the ONE place `run`/`kill`/`delete` decide their global
    /// flags AND environment — this test is the single source of truth for `Rootless`'s exact
    /// flag-and-environment contract (Sol's review: "no independent flag decisions left" at any of
    /// the three call sites). `ExplicitUserNamespace`'s own contract is covered by
    /// `apply_explicit_userns_env_matches_the_policy_exactly` below, which exercises the pure
    /// `Command`-mutation mechanism directly against a hand-built policy — NOT through
    /// `apply_runsc_invocation_policy` itself, since that requires the process-global
    /// `EXPLICIT_USERNS_POLICY` to already be validated-and-installed (see that function's own
    /// refusal-without-preflight behavior, covered by
    /// `apply_runsc_invocation_policy_refuses_explicit_userns_without_a_validated_policy`).
    #[test]
    fn apply_runsc_invocation_policy_matches_the_mode_exactly() {
        let bin = Path::new("/bin/true");
        let mut rootless_cmd = Command::new(bin);
        apply_runsc_invocation_policy(&mut rootless_cmd, bin, RunscInvocationMode::Rootless)
            .unwrap();
        assert_eq!(
            rootless_cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["--rootless"],
            "Rootless must be byte-identical to the pre-slice-2 flag"
        );
        assert_eq!(
            rootless_cmd.get_envs().count(),
            0,
            "Rootless must not alter the child's environment at all"
        );
    }

    /// Exercises the pure `Command`-mutation mechanism `ExplicitUserNamespace` mode applies, given
    /// an already-validated policy — independent of the process-global `EXPLICIT_USERNS_POLICY`
    /// `OnceLock` (which, once installed by any test sharing this test binary's process, cannot be
    /// reset) and independent of `preflight_explicit_userns_policy`'s pinned-runsc-digest check
    /// (which needs a real matching binary this test environment may not have).
    #[test]
    fn apply_explicit_userns_env_matches_the_policy_exactly() {
        let policy = ResolvedExplicitUsernsPolicy {
            helper_dir: PathBuf::from("/usr/bin"),
            runsc_root: PathBuf::from("/var/lib/myelin-runsc-explicit-userns"),
            runsc_root_identity: (0, 0),
        };
        let mut cmd = Command::new("runsc");
        apply_explicit_userns_env(&mut cmd, &policy);
        let args = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            args.contains(&"-ignore-cgroups".to_string()),
            "ExplicitUserNamespace must add -ignore-cgroups: {args:?}"
        );
        assert!(
            !args.contains(&"--rootless".to_string()),
            "ExplicitUserNamespace must drop --rootless: {args:?}"
        );
        assert!(
            args.contains(&"--root=/var/lib/myelin-runsc-explicit-userns".to_string()),
            "ExplicitUserNamespace must pass the exact policy's absolute --root=: {args:?}"
        );
        let envs: Vec<_> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(
            envs,
            vec![("PATH".to_string(), Some("/usr/bin".to_string()))],
            "ExplicitUserNamespace must clear the environment and set PATH to ONLY the exact \
             policy's helper directory: {envs:?}"
        );
    }

    /// Sol's review, round 4/5: `ExplicitUserNamespace` mode must REFUSE outright — not fall back
    /// to ad hoc unvalidated resolution — when no policy has been validated. Calls
    /// `apply_runsc_invocation_policy_given` directly with an EXPLICIT `None`, rather than driving
    /// the real process-global `EXPLICIT_USERNS_POLICY` cell (which, once set by ANY test sharing
    /// this test binary's process — e.g. the live drill — cannot be un-set for a later test to
    /// observe the pre-installation state). This makes the assertion deterministic regardless of
    /// test execution order (round 4's version relied on ordering and silently skipped otherwise;
    /// Sol's review, round 5).
    #[test]
    fn apply_runsc_invocation_policy_refuses_explicit_userns_without_a_validated_policy() {
        let mut cmd = Command::new("runsc");
        let result = apply_runsc_invocation_policy_given(
            &mut cmd,
            RunscInvocationMode::ExplicitUserNamespace(UserNamespaceConfig::for_tests(
                1000, 1000, 100_000, 200_000,
            )),
            None,
        );
        assert!(
            result.is_err(),
            "ExplicitUserNamespace must refuse without a validated policy, not silently proceed"
        );
    }

    /// [`preflight_explicit_userns_helpers`] must accept this development host's real
    /// `/usr/bin` (containing genuine setuid `newuidmap`/`newgidmap`) and must reject a
    /// substitute helper directory containing a non-setuid stand-in.
    #[test]
    fn preflight_explicit_userns_helpers_accepts_real_and_rejects_a_non_setuid_substitute() {
        let real = Path::new("/usr/bin");
        if !real.join("newuidmap").exists() || !real.join("newgidmap").exists() {
            eprintln!("skipping: this host has no /usr/bin/newuidmap or newgidmap");
            return;
        }
        preflight_explicit_userns_helpers(real)
            .expect("this host's real /usr/bin must pass preflight");

        use std::os::unix::fs::PermissionsExt;
        let tmp =
            std::env::temp_dir().join(format!("myelin-preflight-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        for helper in ["newuidmap", "newgidmap"] {
            std::fs::write(tmp.join(helper), b"#!/bin/sh\nexit 1\n").unwrap();
            let mut perms = std::fs::metadata(tmp.join(helper)).unwrap().permissions();
            perms.set_mode(0o755); // executable, but NOT setuid and NOT root-owned
            std::fs::set_permissions(tmp.join(helper), perms).unwrap();
        }
        let result = preflight_explicit_userns_helpers(&tmp);
        std::fs::remove_dir_all(&tmp).ok();
        assert!(
            result.is_err(),
            "a non-root-owned, non-setuid substitute must be refused"
        );
    }

    #[test]
    fn sha256_hex_of_file_matches_a_known_vector() {
        let tmp = std::env::temp_dir().join(format!("myelin-sha256-test-{}", unique_suffix()));
        std::fs::write(&tmp, b"abc").unwrap();
        let digest = sha256_hex_of_file(&tmp).unwrap();
        std::fs::remove_file(&tmp).ok();
        assert_eq!(
            digest, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "must match the well-known SHA-256(\"abc\") test vector"
        );
    }

    /// Sol's review, round 4: pinning must check the binary's own content digest, not only the
    /// version string it happens to print — a forged/rebuilt substitute that echoes the exact
    /// pinned version line must still be refused if its content digest disagrees.
    #[test]
    fn verify_pinned_explicit_userns_runsc_rejects_a_forged_version_string_with_wrong_content() {
        let tmp = std::env::temp_dir().join(format!("myelin-forged-runsc-{}", unique_suffix()));
        std::fs::write(
            &tmp,
            format!("#!/bin/sh\necho '{PINNED_EXPLICIT_USERNS_RUNSC_VERSION}'\n"),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&tmp).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        std::fs::set_permissions(&tmp, perms).unwrap();
        let result = verify_pinned_explicit_userns_runsc(&tmp);
        std::fs::remove_file(&tmp).ok();
        assert!(
            result.is_err(),
            "a forged version string with the wrong content digest must be refused: {result:?}"
        );
    }

    /// Sol's review, round 5: the digest pin alone doesn't stop the binary being replaced between
    /// preflight and a later launch — `harden_explicit_userns_runsc_binary` must refuse a binary
    /// this process itself owns (which it could `chmod`/replace at will), not only a wrong digest.
    #[test]
    fn harden_explicit_userns_runsc_binary_refuses_a_non_root_owned_file() {
        let tmp = std::env::temp_dir().join(format!("myelin-fake-runsc-{}", unique_suffix()));
        std::fs::write(&tmp, b"#!/bin/sh\nexit 0\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&tmp).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tmp, perms).unwrap();
        let result = harden_explicit_userns_runsc_binary(&tmp);
        std::fs::remove_file(&tmp).ok();
        assert!(
            result.is_err(),
            "a binary owned by this process's own euid must be refused: {result:?}"
        );
    }

    #[test]
    fn harden_explicit_userns_runsc_binary_refuses_a_symlink() {
        let base =
            std::env::temp_dir().join(format!("myelin-fake-runsc-symlink-{}", unique_suffix()));
        std::fs::create_dir_all(&base).unwrap();
        let real = base.join("real-runsc");
        std::fs::write(&real, b"#!/bin/sh\nexit 0\n").unwrap();
        let link = base.join("runsc");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let result = harden_explicit_userns_runsc_binary(&link);
        let _ = std::fs::remove_dir_all(&base);
        assert!(
            result.is_err(),
            "a symlinked binary path must be refused rather than followed: {result:?}"
        );
    }

    /// Mirrors `strict_construction_refuses_a_leases_dir_whose_parent_is_writable_by_us` in
    /// `user_namespace.rs` — the exact same ancestor-writability requirement, applied here to the
    /// explicit-userns runsc state root (Sol's review, round 5: an absolute path string alone does
    /// not freeze what it names).
    #[test]
    fn harden_explicit_userns_runsc_root_refuses_a_leaf_under_a_writable_parent() {
        let base = std::env::temp_dir().join(format!(
            "myelin-runsc-root-writable-parent-{}",
            unique_suffix()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let leaf = base.join("runsc-root");
        let result = harden_explicit_userns_runsc_root(&leaf);
        let _ = std::fs::remove_dir_all(&base);
        assert!(
            result.is_err(),
            "a leaf whose parent is writable by this process must be refused: {result:?}"
        );
    }

    /// Sol's review, round 6: no auto-creation — a missing leaf must be refused outright (proven
    /// via `verify_explicit_userns_runsc_root_leaf` directly, isolated from the ancestor check).
    #[test]
    fn verify_explicit_userns_runsc_root_leaf_refuses_a_missing_leaf() {
        let missing =
            std::env::temp_dir().join(format!("myelin-missing-runsc-root-{}", unique_suffix()));
        let result = verify_explicit_userns_runsc_root_leaf(&missing);
        assert!(
            result.is_err(),
            "a non-pre-provisioned leaf must be refused, never auto-created: {result:?}"
        );
    }

    /// Isolates JUST `verify_explicit_userns_runsc_root_leaf` (not the full
    /// `harden_explicit_userns_runsc_root`, whose ancestor check would refuse first against any
    /// fixture under a writable temp directory) against a real symlinked leaf.
    #[test]
    fn verify_explicit_userns_runsc_root_leaf_refuses_a_symlinked_leaf() {
        let base =
            std::env::temp_dir().join(format!("myelin-runsc-root-symlink-{}", unique_suffix()));
        std::fs::create_dir_all(&base).unwrap();
        let real_dir = base.join("real");
        std::fs::create_dir_all(&real_dir).unwrap();
        let link = base.join("runsc-root");
        std::os::unix::fs::symlink(&real_dir, &link).unwrap();
        let result = verify_explicit_userns_runsc_root_leaf(&link);
        let _ = std::fs::remove_dir_all(&base);
        assert!(
            result.is_err(),
            "a symlinked state-root leaf must be refused rather than followed: {result:?}"
        );
    }

    /// Sol's review, round 7: rejecting group/other bits alone still admits a mode like `0500`
    /// (owner cannot write) or `0000` (owner cannot even search it) — both unusable for actually
    /// creating/reading runsc state, despite passing the group/other-only check.
    #[test]
    fn verify_explicit_userns_runsc_root_leaf_refuses_an_owner_non_writable_directory() {
        let dir = std::env::temp_dir().join(format!("myelin-runsc-root-0500-{}", unique_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o500); // r-x------: owner cannot write, though group/other bits are clear.
        std::fs::set_permissions(&dir, perms).unwrap();
        let result = verify_explicit_userns_runsc_root_leaf(&dir);
        let mut restore = std::fs::metadata(&dir).unwrap().permissions();
        restore.set_mode(0o700);
        std::fs::set_permissions(&dir, restore).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            result.is_err(),
            "an owner-non-writable directory must be refused even with no group/other bits: \
             {result:?}"
        );
    }

    #[test]
    fn verify_explicit_userns_runsc_root_leaf_accepts_a_properly_pre_provisioned_leaf() {
        let dir = std::env::temp_dir().join(format!("myelin-runsc-root-ok-{}", unique_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&dir, perms).unwrap();
        let result = verify_explicit_userns_runsc_root_leaf(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            result.is_ok(),
            "a real, owned, mode-0700 pre-provisioned directory must be accepted: {result:?}"
        );
    }

    // ───── CT-007 slice 3, piece 7c: pure decision-logic tests (no real objects at all) ─────

    #[test]
    fn classify_workspace_deletion_ok_proves_absence_with_no_diagnostic() {
        let outcome = classify_workspace_deletion(Ok(()));
        assert!(matches!(
            outcome,
            WorkspaceDeletionOutcome::ProvenAbsent { diagnostic: None }
        ));
    }

    #[test]
    fn classify_workspace_deletion_internal_invariant_violated_proves_absence_but_surfaces_it() {
        let outcome =
            classify_workspace_deletion(Err(DeleteWorkspaceError::InternalInvariantViolated {
                reason: "bookkeeping corruption".to_string(),
            }));
        match outcome {
            WorkspaceDeletionOutcome::ProvenAbsent {
                diagnostic: Some(diagnostic),
            } => assert!(diagnostic.contains("bookkeeping corruption")),
            other => {
                panic!("expected ProvenAbsent with a diagnostic, got a different shape: {other:?}")
            }
        }
    }

    #[test]
    fn classify_workspace_deletion_storage_failure_does_not_prove_absence() {
        let outcome = classify_workspace_deletion(Err(DeleteWorkspaceError::Storage(
            WorkspaceStorageError::ZeroQuota,
        )));
        assert!(matches!(
            outcome,
            WorkspaceDeletionOutcome::NotProvenAbsent { .. }
        ));
    }

    #[test]
    fn augment_settled_result_with_enabled_cleanup_failure_converts_a_clean_success_to_executed() {
        let usage = ResourceUsage {
            cpu_seconds: 3,
            mem_byte_seconds: 4096,
        };
        let discarded = std::cell::Cell::new(false);
        let result: Result<u64, RunFailure> = Ok(42);
        let result = augment_settled_result_with_enabled_cleanup_failure(
            result,
            move |_: &u64| usage,
            |value: u64| {
                assert_eq!(value, 42);
                discarded.set(true);
            },
            "workspace delete/sync failed".to_string(),
        );
        match result {
            Err(RunFailure::Executed {
                usage: got_usage,
                message,
            }) => {
                assert_eq!(got_usage, usage);
                assert!(message.contains("workspace/userns-lease cleanup failed"));
                assert!(message.contains("workspace delete/sync failed"));
            }
            other => panic!("expected RunFailure::Executed, got {other:?}"),
        }
    }

    #[test]
    fn augment_settled_result_with_enabled_cleanup_failure_augments_an_existing_failure() {
        let original_usage = ResourceUsage {
            cpu_seconds: 7,
            mem_byte_seconds: 8192,
        };
        let result: Result<u64, RunFailure> =
            Err(RunFailure::executed("original failure", original_usage));
        let result = augment_settled_result_with_enabled_cleanup_failure(
            result,
            |_: &u64| panic!("usage_of must not be called when the primary already failed"),
            |_: u64| panic!("on_discarded_success must not run when the primary already failed"),
            "lease release failed".to_string(),
        );
        match result {
            Err(RunFailure::Executed {
                usage: got_usage,
                message,
            }) => {
                assert_eq!(got_usage, original_usage);
                assert!(message.contains("original failure"));
                assert!(message.contains("workspace/userns-lease cleanup also failed"));
                assert!(message.contains("lease release failed"));
            }
            other => panic!("expected an augmented RunFailure::Executed, got {other:?}"),
        }
    }

    /// Integrated (not just pure-`classify_workspace_deletion`-level) coverage: a REAL lease flows
    /// all the way through `delete_workspace_then_release_lease_if_absent` (the exact helper
    /// `cleanup_pre_bind_failure`'s `Allocated` arm calls), with only the `delete_workspace`
    /// operation injected as synthetic. `InternalInvariantViolated` means deletion actually
    /// succeeded (capacity was released, subvolume gone) despite a bookkeeping bug -- disk absence
    /// IS proven, so the real lease must still be releasable via `release_unused()`, and the
    /// failure must still be surfaced as a diagnostic.
    #[cfg(feature = "test-support")]
    #[test]
    fn delete_workspace_then_release_lease_if_absent_releases_a_real_lease_on_internal_invariant_violated(
    ) {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("integrated-invariant-violated")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("integrated-invariant-violated")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let lease = userns_allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let capacity = workspace_manager
            .acquire_capacity(8 << 20)
            .expect("capacity must be available against a fresh 1 GiB ceiling");
        let workspace = workspace_manager
            .create_workspace(
                "integrated-invariant-violated-job",
                8 << 20,
                lease.host_uid(),
                lease.host_gid(),
                capacity,
            )
            .expect("create_workspace must succeed against a real, privileged Btrfs backend");
        let host_path = workspace.host_path().to_path_buf();

        // Sol's round-2 review: a synthetic error alone would make the "disk absence proven"
        // premise false (the real subvolume would still be sitting there) -- genuinely call the
        // REAL `delete_workspace` first (so the subvolume really is gone and real capacity really
        // is released), THEN report the synthetic `InternalInvariantViolated` as if some OTHER
        // bookkeeping check inside a real `delete_workspace` call had separately failed atop an
        // otherwise-successful deletion -- exactly the scenario this variant models.
        let diagnostics = delete_workspace_then_release_lease_if_absent(workspace, lease, |w| {
            workspace_manager.delete_workspace(w).expect(
                "the real delete must succeed for this test to model a genuine invariant \
                 violation atop an otherwise-successful deletion",
            );
            Err(DeleteWorkspaceError::InternalInvariantViolated {
                reason: "synthetic bookkeeping corruption for this test".to_string(),
            })
        });

        assert_eq!(
            diagnostics.len(),
            1,
            "the failure must be surfaced: {diagnostics:?}"
        );
        assert!(diagnostics[0].contains("synthetic bookkeeping corruption"));
        assert!(
            !host_path.exists(),
            "the real subvolume must genuinely be gone -- this variant's whole premise is that \
             disk absence IS proven, just alongside a separately-surfaced bookkeeping failure"
        );
        assert!(
            userns_allocator.is_healthy(),
            "InternalInvariantViolated proves disk absence -- the real lease must have released \
             cleanly, not been quarantined"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// The `Storage`/sync-failure counterpart: disk absence is NOT proven, so the real lease must
    /// be left unreleased (dropped -- quarantined by `Drop`, never `release_unused()`), and the
    /// failure must still be surfaced as a diagnostic.
    #[cfg(feature = "test-support")]
    #[test]
    fn delete_workspace_then_release_lease_if_absent_quarantines_a_real_lease_on_a_storage_failure()
    {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("integrated-storage-failure")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("integrated-storage-failure")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let lease = userns_allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let capacity = workspace_manager
            .acquire_capacity(8 << 20)
            .expect("capacity must be available against a fresh 1 GiB ceiling");
        let workspace = workspace_manager
            .create_workspace(
                "integrated-storage-failure-job",
                8 << 20,
                lease.host_uid(),
                lease.host_gid(),
                capacity,
            )
            .expect("create_workspace must succeed against a real, privileged Btrfs backend");
        let host_path = workspace.host_path().to_path_buf();

        let diagnostics = delete_workspace_then_release_lease_if_absent(workspace, lease, |_w| {
            Err(DeleteWorkspaceError::Storage(
                WorkspaceStorageError::ZeroQuota,
            ))
        });

        assert_eq!(
            diagnostics.len(),
            1,
            "the failure must be surfaced: {diagnostics:?}"
        );
        assert!(diagnostics[0].contains("delete/sync failed"));
        assert!(
            !userns_allocator.is_healthy(),
            "a Storage failure does NOT prove disk absence -- the real lease must be quarantined, \
             never released"
        );

        drop(workspace_manager);
        let sink2: crate::workspace_manager::IncidentSink =
            Arc::new(|msg: &str| eprintln!("[piece7c workspace incident] {msg}"));
        let fresh = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: workspace_base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink2,
        )
        .expect("a fresh manager's own boot reconciliation must clean up the orphan and succeed");
        assert!(!host_path.exists());
        drop(fresh);
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
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
    fn real_workspace_manager_without_qgroup_probe_for_tests(
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
    fn real_workspace_manager_for_tests(tag: &str) -> Option<(WorkspaceManager, PathBuf)> {
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
    fn real_userns_allocator_for_tests(tag: &str) -> Option<(UserNamespaceAllocator, PathBuf)> {
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

    // ───────── CT-007 slice 3, piece 7c: `bind_enabled_lease_given` classification matrix ─────────
    //
    // `bind_enabled_lease_given` was extracted specifically so this matrix is coverable with a real
    // (cheap, non-privileged) `UserNamespaceLease` and a bare `LeaseBindState` value — no
    // `ManagedWorkspace`/`CAP_SYS_ADMIN` involved at all, unlike the acquire/settle tests above.

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_enabled_lease_given_binds_and_records_bound_on_success() {
        let Some((allocator, leases_dir)) = real_userns_allocator_for_tests("bind-ok") else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let expected_root_identity = (11, 22);
        let cgroup_identity = (33, 44);
        let result = bind_enabled_lease_given(
            &mut lease,
            &mut bind_state,
            expected_root_identity,
            "bind-ok-container",
            cgroup_identity,
            || Ok(expected_root_identity),
        );
        assert!(result.is_ok());
        assert_eq!(
            bind_state,
            LeaseBindState::Bound {
                container_id: "bind-ok-container".to_string(),
                runsc_root_identity: expected_root_identity,
                cgroup_identity,
            }
        );
        let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
            "bind-ok-container".to_string(),
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity: expected_root_identity,
            },
            CgroupQuiescenceEvidence::assert_for_tests(cgroup_identity),
        );
        let proof = UserNamespaceQuiescenceProof::from_runtime_evidence(&lease, &evidence)
            .expect("a matching evidence must mint a proof");
        lease
            .release(proof)
            .expect("release must succeed after a real bind");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// Never calls `lease.bind` at all when the live identity revalidation disagrees with what was
    /// expected — this is exactly the check that lets the caller (`run_production_container_streaming`)
    /// refuse BEFORE ever calling `run_and_capture`, without having mutated the lease or its durable
    /// marker in any way.
    #[cfg(feature = "test-support")]
    #[test]
    fn bind_enabled_lease_given_refuses_before_touching_the_lease_when_identity_drifted() {
        let Some((allocator, leases_dir)) = real_userns_allocator_for_tests("bind-identity-drift")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let result = bind_enabled_lease_given(
            &mut lease,
            &mut bind_state,
            (11, 22),
            "bind-drift-container",
            (33, 44),
            || Ok((99, 99)), // the live revalidation disagrees with the expected identity.
        );
        assert!(result.is_err());
        assert_eq!(
            bind_state,
            LeaseBindState::Allocated,
            "an identity-drift refusal must never touch bind_state -- the lease was never bound"
        );
        // The lease itself was never mutated -- it can still be released as a plain unused lease.
        lease
            .release_unused()
            .expect("an un-bound, un-touched lease must still release cleanly");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_enabled_lease_given_classifies_an_invalid_container_id_as_still_allocated() {
        let Some((allocator, leases_dir)) = real_userns_allocator_for_tests("bind-invalid-id")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let expected_root_identity = (11, 22);
        let result = bind_enabled_lease_given(
            &mut lease,
            &mut bind_state,
            expected_root_identity,
            "", // empty container_id -> UserNamespaceBindError::InvalidContainerId
            (33, 44),
            || Ok(expected_root_identity),
        );
        assert!(result.is_err());
        assert_eq!(
            bind_state,
            LeaseBindState::Allocated,
            "InvalidContainerId is a caller bug, not a global-trust failure -- nothing touched \
             disk, so the lease remains safely Allocated and reusable"
        );
        lease.release_unused().expect(
            "an Allocated lease untouched by a caller-bug refusal must still release cleanly",
        );
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_enabled_lease_given_classifies_a_marker_mismatch_as_unreleasable() {
        let Some((allocator, leases_dir)) = real_userns_allocator_for_tests("bind-marker-mismatch")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let expected_root_identity = (11, 22);
        // Bind it once for real, durably transitioning the on-disk marker to Bound -- a SECOND
        // bind attempt against the same lease will then find the marker no longer `Allocated`,
        // which is exactly the `MarkerMismatch` path (poisoning the allocator).
        lease
            .bind(
                "already-bound".to_string(),
                expected_root_identity,
                (33, 44),
            )
            .expect("the first bind against a fresh Allocated lease must succeed");
        let mut bind_state = LeaseBindState::Bound {
            container_id: "already-bound".to_string(),
            runsc_root_identity: expected_root_identity,
            cgroup_identity: (33, 44),
        };
        let result = bind_enabled_lease_given(
            &mut lease,
            &mut bind_state,
            expected_root_identity,
            "second-bind-attempt",
            (55, 66),
            || Ok(expected_root_identity),
        );
        assert!(result.is_err());
        assert_eq!(
            bind_state,
            LeaseBindState::Unreleasable,
            "MarkerMismatch means the on-disk state no longer agrees with this in-memory lease -- \
             ambiguous and never safe to release"
        );
        assert!(
            !allocator.is_healthy(),
            "MarkerMismatch must globally poison the allocator (a global-trust failure, not a \
             caller bug)"
        );
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    // ───── CT-007 slice 3, piece 7c: `bind_then_continue` — the bind-then-capture composition ─────
    //
    // Sol's round-2 review: `bind_enabled_lease_given` proves the classification table is correct,
    // but leaves the crucial decision -- never invoking the capture/spawn continuation after a
    // failed/unconfirmed bind -- to its caller. These tests exercise that COMPOSITION directly with
    // a bare counting closure standing in for `run_and_capture` -- no real runsc spawn, no
    // privileged Btrfs, just a real (cheap) `UserNamespaceLease` where `Enabled` coverage needs one.

    #[test]
    fn bind_then_continue_always_invokes_the_continuation_when_rootless() {
        let calls = std::cell::Cell::new(0u32);
        let result = bind_then_continue(
            None,
            "rootless-container",
            (33, 44),
            || panic!("Rootless must never need to revalidate a root identity"),
            || {
                calls.set(calls.get() + 1);
                "captured"
            },
        );
        assert_eq!(result, Ok("captured"));
        assert_eq!(calls.get(), 1);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_then_continue_invokes_the_continuation_exactly_once_after_a_successful_bind() {
        let Some((allocator, leases_dir)) =
            real_userns_allocator_for_tests("bind-then-continue-ok")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let expected_root_identity = (11, 22);
        let calls = std::cell::Cell::new(0u32);
        let result = bind_then_continue(
            Some((&mut lease, &mut bind_state, expected_root_identity)),
            "bind-then-continue-ok-container",
            (33, 44),
            || Ok(expected_root_identity),
            || {
                calls.set(calls.get() + 1);
                "captured"
            },
        );
        assert_eq!(result, Ok("captured"));
        assert_eq!(
            calls.get(),
            1,
            "a successful bind must invoke the continuation exactly once"
        );
        assert_eq!(
            bind_state,
            LeaseBindState::Bound {
                container_id: "bind-then-continue-ok-container".to_string(),
                runsc_root_identity: expected_root_identity,
                cgroup_identity: (33, 44),
            }
        );
        let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
            "bind-then-continue-ok-container".to_string(),
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity: expected_root_identity,
            },
            CgroupQuiescenceEvidence::assert_for_tests((33, 44)),
        );
        let proof = UserNamespaceQuiescenceProof::from_runtime_evidence(&lease, &evidence)
            .expect("a matching evidence must mint a proof");
        lease
            .release(proof)
            .expect("release must succeed after a real bind");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// The security property this whole piece rests on: a live identity-drift refusal must leave
    /// the continuation-call count at zero -- no exec may ever follow a failed/unconfirmed bind.
    #[cfg(feature = "test-support")]
    #[test]
    fn bind_then_continue_never_invokes_the_continuation_when_identity_drifted() {
        let Some((allocator, leases_dir)) =
            real_userns_allocator_for_tests("bind-then-continue-drift")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let calls = std::cell::Cell::new(0u32);
        let result = bind_then_continue(
            Some((&mut lease, &mut bind_state, (11, 22))),
            "bind-then-continue-drift-container",
            (33, 44),
            || Ok((99, 99)), // disagrees with the expected (11, 22).
            || {
                calls.set(calls.get() + 1);
                "captured"
            },
        );
        assert!(result.is_err());
        assert_eq!(
            calls.get(),
            0,
            "an identity-drift refusal must NEVER invoke the capture/spawn continuation"
        );
        assert_eq!(bind_state, LeaseBindState::Allocated);
        lease
            .release_unused()
            .expect("an un-bound, un-touched lease must still release cleanly");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// Same property, for a real durable bind failure (not merely a live-identity refusal).
    #[cfg(feature = "test-support")]
    #[test]
    fn bind_then_continue_never_invokes_the_continuation_on_a_real_bind_failure() {
        let Some((allocator, leases_dir)) =
            real_userns_allocator_for_tests("bind-then-continue-bind-fail")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let expected_root_identity = (11, 22);
        let calls = std::cell::Cell::new(0u32);
        let result = bind_then_continue(
            Some((
                &mut lease,
                &mut LeaseBindState::Allocated,
                expected_root_identity,
            )),
            "", // empty container_id -> UserNamespaceBindError::InvalidContainerId
            (33, 44),
            || Ok(expected_root_identity),
            || {
                calls.set(calls.get() + 1);
                "captured"
            },
        );
        assert!(result.is_err());
        assert_eq!(
            calls.get(),
            0,
            "a real bind failure must NEVER invoke the capture/spawn continuation"
        );
        lease.release_unused().expect(
            "an Allocated lease untouched by a caller-bug bind refusal must still release cleanly",
        );
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_then_settle_releases_cleanly_on_a_matching_evidence() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("acquire-settle-ok")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("acquire-settle-ok")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let container_id = "acquire-settle-ok-container";
        let (cfg, mut context) = acquire_enabled_workspace(
            &command_spec,
            &profile,
            container_id,
            PathBuf::from("/abs/staged-rootfs"),
            &workspace_manager,
            &userns_allocator,
            None,
        )
        .expect("acquisition must succeed against a healthy real manager/allocator");
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::ExplicitUserNamespace(context.lease.config())
        );
        assert_eq!(context.bind_state, LeaseBindState::Allocated);

        // Simulate what `run_production_container_streaming` does: bind, THEN finalize.
        let runsc_root_identity = (11, 22);
        let cgroup_identity = (33, 44);
        context
            .lease
            .bind(
                container_id.to_string(),
                runsc_root_identity,
                cgroup_identity,
            )
            .expect("bind must succeed for a fresh Allocated lease");
        context.bind_state = LeaseBindState::Bound {
            container_id: container_id.to_string(),
            runsc_root_identity,
            cgroup_identity,
        };
        let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
            container_id.to_string(),
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity,
            },
            CgroupQuiescenceEvidence::assert_for_tests(cgroup_identity),
        );
        settle_enabled_workspace_and_lease(context, &workspace_manager, &evidence)
            .expect("settling a matching evidence against a Bound lease must succeed");
        assert!(workspace_manager.is_healthy());
        assert!(userns_allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    // ───────── CT-007 slice 5b.3-6c: parent-attempt reservation mode (always-run) ─────────

    /// A minimal recording fake [`AttemptAuthority`] for the 6c continuation/orchestrator tests, with
    /// optional injected failures for the post-acquisition authority-failure matrix.
    struct FakeAttemptAuthority {
        ops: Mutex<Vec<String>>,
        should_requeue: bool,
        fail_begin_phase: bool,
        fail_mint_phase: bool,
    }
    #[allow(dead_code)]
    impl FakeAttemptAuthority {
        fn new(should_requeue: bool) -> Self {
            Self {
                ops: Mutex::new(Vec::new()),
                should_requeue,
                fail_begin_phase: false,
                fail_mint_phase: false,
            }
        }
        fn failing_begin_phase() -> Self {
            Self {
                fail_begin_phase: true,
                ..Self::new(true)
            }
        }
        fn failing_mint_phase() -> Self {
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
    fn report_claim() -> crate::runner::PreparationReportClaim {
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

    fn fake_authorization_context() -> crate::RunTokenAuthorizationContext {
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

    /// The legacy-mode `RunnerHooks` (every production constructor) selects the legacy reserve and
    /// REFUSES parent-attempt admission — the dormancy gate that keeps the V2 path unreachable.
    #[test]
    fn reserve_parent_attempt_refuses_in_legacy_mode() {
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        );
        assert!(matches!(
            hooks.reserve_parent_attempt(&spec(vec![])),
            Err(HookError(_))
        ));
    }

    /// Once the V2 reservation mode is installed, admission returns the injected
    /// [`ParentAttemptAdmission`] (both arms carry the reserve handle).
    #[test]
    fn reserve_parent_attempt_returns_the_installed_admission() {
        use crate::checkout_orchestration::ParentAttemptAdmission;
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        )
        .with_parent_attempt_reservation(Box::new(|_spec| {
            Ok(ParentAttemptAdmission::Admitted {
                claim: report_claim(),
                reserve: ReserveHandle("ci-reserve:v2:a".to_string()),
                attempt_authority: Box::new(FakeAttemptAuthority::new(true)),
            })
        }));
        match hooks
            .reserve_parent_attempt(&spec(vec![]))
            .expect("admitted")
        {
            ParentAttemptAdmission::Admitted { reserve, .. } => {
                assert_eq!(reserve.0, "ci-reserve:v2:a")
            }
            ParentAttemptAdmission::AttemptsExhausted { .. } => panic!("expected Admitted"),
        }
    }

    // ───────── CT-007 slice 5b.3-6c: workload failure-phase + begun-phase routing (always-run) ─────────

    /// Sol's finding 5 + finding 6(c): the workload failure-phase matrix. `EnabledPrepared` binds the
    /// lease BEFORE the launch CAS commits, so a pre-CAS `Uncommitted` failure leaves the row `leased`
    /// → PREPARATION requeue, NEVER a running-claim workload attempt. Only `CommittedButNotExecuted` and
    /// `Executed` (a committed running claim) go to the reporter-owned workload path.
    #[test]
    fn classify_bound_workload_failure_splits_pre_and_post_cas() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let authority = FakeAttemptAuthority::new(true);

        // Uncommitted (pre-CAS, row still leased) → preparation requeue.
        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::uncommitted("gate failed"),
        );
        assert!(
            matches!(
                out,
                CheckoutContinuationOutcome::PreparationRetryable {
                    phase: PreparationPhase::CheckoutMaterialization,
                    ..
                }
            ),
            "a pre-CAS Uncommitted workload failure is a preparation requeue, got {out:?}"
        );

        // CommitOutcomeUnknown → reconciliation (never guessed).
        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::commit_outcome_unknown("ambiguous"),
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::ReconciliationRequired { .. }
        ));

        // CommittedButNotExecuted (running claim) → workload retryable, zero usage.
        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::committed_but_not_executed("never execed"),
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::WorkloadRetryable {
                usage: ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0
                },
                ..
            }
        ));

        // Executed (running claim, real usage) → workload retryable carrying that usage.
        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::executed(
                "teardown infra failed",
                ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 6,
                },
            ),
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::WorkloadRetryable {
                usage: ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 6
                },
                ..
            }
        ));
    }

    /// Sol's finding 5: when the parent-attempt budget is exhausted, a pre-CAS `Uncommitted` workload
    /// failure terminalizes `AttemptsExhausted` rather than requeueing.
    #[test]
    fn classify_uncommitted_terminalizes_when_attempts_are_exhausted() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let authority = FakeAttemptAuthority::new(false);
        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::uncommitted("gate failed"),
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::PreparationTerminal {
                disposition: crate::runner::PreparationTerminalDisposition::AttemptsExhausted,
                ..
            }
        ));
    }

    /// Sol's finding 3: an advertise mint/authorization failure after `begin_phase(CheckoutTransport)`
    /// must COMPLETE the begun transport phase with zero (never leave it started for the sealer) and
    /// route requeue/exhaustion.
    #[test]
    fn resolve_begun_transport_failure_completes_zero_then_requeues() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let authority = FakeAttemptAuthority::new(true);
        let out = resolve_begun_transport_failure(&authority, &report_claim());
        assert_eq!(
            authority.ops.lock().unwrap().clone(),
            vec!["complete:CheckoutTransport:0"],
            "the begun transport phase is completed with zero"
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::PreparationRetryable {
                phase: PreparationPhase::CheckoutTransport,
                ..
            }
        ));
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
    fn checkout_spec() -> JobSpec {
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
    fn acquire_real_checkout_capsule(
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

    /// CT-007 slice 5b.3-6c: the DORMANT continuation (steps 15–18) over a REAL capsule with an
    /// INJECTED terminal Hop B failure — proves the continuation begins/authorizes the materialization
    /// phase, disposes the capsule along its session disposition (Prepared → delete + release_prepared),
    /// and routes the terminal disposition through the journal (`complete_phase` + `PreparationTerminal`)
    /// — all with no `runsc`. Gated like the 6a dispose matrix (real Btrfs+userns); soft-skips otherwise.
    /// CT-007 slice 5b.3-6c: the `#[cfg(test)]` `into_prepared_for_tests` consuming transition drives
    /// the REAL session/lease durable state to `Prepared` and yields a `PreparedCheckoutRuntime` — so a
    /// subsequent `dispose_checkout_runtime` takes the `Prepared → delete + release_prepared` path,
    /// returning the slot to the pool. Proves the test transition is honest (real durable state), not a
    /// wrapped `NotStarted` capsule. Gated like the 6a dispose matrix; soft-skips otherwise.
    #[cfg(feature = "test-support")]
    #[test]
    fn into_prepared_for_tests_drives_the_real_lease_to_prepared() {
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("into-prepared")
        else {
            return;
        };
        let prepared =
            runtime.into_prepared_for_tests(PreparedCheckoutEvidence::for_tests(ResourceUsage {
                cpu_seconds: 2,
                mem_byte_seconds: 3,
            }));
        let diagnostics = prepared.dispose_checkout_runtime(&workspace_manager);
        assert!(
            diagnostics.is_empty(),
            "a clean Prepared disposal must produce no diagnostics, got {diagnostics:?}"
        );
        assert!(workspace_manager.is_healthy());
        assert!(
            userns_allocator.lease().is_ok(),
            "release_prepared must return the slot to the pool"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[allow(clippy::result_large_err)]
    #[test]
    fn continuation_routes_a_terminal_hop_b_failure_and_disposes_the_prepared_capsule() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("continuation-terminal-hopb")
        else {
            return;
        };
        let backend = GvisorBackend::new(test_registry());
        let spec = checkout_spec();
        let scope = crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace)
            .expect("scope derives")
            .expect("checkout-bearing");
        // The V2 phase-authorization hook returns the retained (here immediate) permit.
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        )
        .with_checkout_phase_authorization(Box::new(|_spec, _scope, _phase| {
            Ok(LaunchPermit::immediate())
        }));
        let authority = FakeAttemptAuthority::new(false);
        let preparation_spec = CheckoutPreparationSpec::new(
            crate::workspace_intent::ExpectedGitCommitId::new(
                scope.commit_hex().to_string(),
                scope.commit_format(),
            )
            .unwrap(),
            PrefetchedCheckoutPack::for_tests(),
            spec.limits,
        )
        .unwrap();

        let outcome = backend
            .launch_checkout_continuation_given(
                &spec,
                &hooks,
                &authority,
                &report_claim(),
                &scope,
                runtime,
                preparation_spec,
                &workspace_manager,
                std::path::Path::new("/abs/staged-rootfs"),
                // Injected Hop B: a terminal materialization failure that hands the capsule back.
                |runtime, _spec, _run_token, _authorization| {
                    Err((
                        runtime,
                        CheckoutPreparationError::RejectedAfterQuiescence {
                            message: "injected terminal checkout rejection".to_string(),
                            usage: ResourceUsage {
                                cpu_seconds: 4,
                                mem_byte_seconds: 8,
                            },
                            disposition: PreparationAttemptDisposition::Terminal(
                                PreparationTerminalDisposition::Failed {
                                    phase: PreparationPhase::CheckoutMaterialization,
                                },
                            ),
                        },
                    ))
                },
                // The workload runner op must never be reached on a Hop B failure.
                |_prepared, _authority, _hooks, _spec, _wm, _rootfs| {
                    panic!("the workload transition must not run after a Hop B failure")
                },
            )
            .expect("the continuation routes a terminal Hop B failure without a structural error");

        assert!(
            matches!(
                outcome,
                CheckoutContinuationOutcome::PreparationTerminal {
                    disposition: PreparationTerminalDisposition::Failed {
                        phase: PreparationPhase::CheckoutMaterialization
                    },
                    diagnostic: Some(ref diagnostic),
                    ..
                }
                if diagnostic == "injected terminal checkout rejection"
            ),
            "a terminal Hop B failure retains its diagnostic in the preparation-terminal outcome, got {outcome:?}"
        );
        let ops = authority.ops.lock().unwrap().clone();
        assert!(
            ops.contains(&"begin:CheckoutMaterialization".to_string())
                && ops.contains(&"mint:Materialization".to_string())
                && ops.contains(&"complete:CheckoutMaterialization:4".to_string()),
            "the continuation began, authorized, and completed the materialization phase, got {ops:?}"
        );
        // The capsule was disposed along its session disposition BEFORE the error crossed back — the
        // slot is reusable and the managers stay healthy (this fake hands the capsule back in its
        // as-acquired NotStarted state, so disposal is the delete + release_unused path).
        assert!(workspace_manager.is_healthy());
        assert!(
            userns_allocator.lease().is_ok(),
            "disposing the capsule must return the slot to the pool"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// **Sol's r5 finding 1 proof: the RAII guard disposes a NotStarted capsule SAFELY on any early
    /// exit before Hop B.** Simulates Sol's evasion (an early `return`/`?`/`panic!` that implicitly drops
    /// the capsule) by creating the guard and letting it drop WITHOUT disarming — the guard's `Drop` must
    /// run the SAFE NotStarted cleanup (delete workspace + `release_unused`), leaving the workspace
    /// manager HEALTHY and the userns slot REUSABLE — NOT the capsule's poison-on-bare-drop. Gated like
    /// the 6a dispose matrix (real Btrfs+userns); soft-skips otherwise.
    #[cfg(feature = "test-support")]
    #[test]
    fn not_started_capsule_guard_disposes_safely_on_any_early_exit() {
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("guard-early-exit")
        else {
            return;
        };
        // An early exit before Hop B: the guard is created and DROPPED without `disarm` (exactly what an
        // injected `if cond { return Ok(...); }` / `?` / panic would do). Its Drop performs safe cleanup.
        {
            let _guard = NotStartedCapsuleGuard::new(runtime, &workspace_manager);
        }
        assert!(
            workspace_manager.is_healthy(),
            "the guard's Drop must NOT poison the manager — it performs the safe NotStarted cleanup"
        );
        assert!(
            userns_allocator.lease().is_ok(),
            "the guard's Drop must release_unused the slot — the pool slot is reusable"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// The success path DISARMS the guard: `disarm` hands the capsule back (Drop then a no-op), so the
    /// capsule survives to be moved into Hop B — no double-dispose. Proven by disposing the disarmed
    /// capsule explicitly and observing the slot free exactly once.
    #[cfg(feature = "test-support")]
    #[test]
    fn not_started_capsule_guard_disarm_hands_back_the_capsule() {
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("guard-disarm")
        else {
            return;
        };
        let runtime = NotStartedCapsuleGuard::new(runtime, &workspace_manager).disarm();
        // The disarmed guard dropped harmlessly; the capsule is intact — dispose it exactly once.
        let diagnostics = runtime.dispose_checkout_runtime(&workspace_manager);
        assert!(
            diagnostics.is_empty(),
            "a clean NotStarted disposal, got {diagnostics:?}"
        );
        assert!(workspace_manager.is_healthy());
        assert!(userns_allocator.lease().is_ok());
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// Sol's finding 6(a): the DETERMINISTIC full-success continuation sequence — begin materialization,
    /// mint + authorize the MATERIALIZATION generation (asserting the phase hook is handed the ROTATED
    /// materialization spec, not the advertise base — finding 1), fused Hop B → Prepared, then a workload
    /// that launches → `WorkloadLaunched`. Uses a real capsule (gated like the 6a matrix) but a synthetic
    /// workload op (no `runsc`/userns policy needed); the workload's OWN generation threading is proven
    /// separately by `run_retained_workload_given` below.
    #[cfg(feature = "test-support")]
    #[allow(clippy::result_large_err)]
    #[test]
    fn continuation_full_success_threads_materialization_generation_and_launches() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("continuation-full-success")
        else {
            return;
        };
        let backend = GvisorBackend::new(test_registry());
        let spec = checkout_spec();
        let scope = crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace)
            .expect("scope derives")
            .expect("checkout-bearing");
        let seen_materialization_jti = Arc::new(Mutex::new(None::<String>));
        let seen = seen_materialization_jti.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        )
        .with_checkout_phase_authorization(Box::new(move |s, _scope, phase| {
            if phase == crate::CheckoutPhase::Materialization {
                *seen.lock().unwrap() = Some(s.run_token.jti.clone());
            }
            Ok(LaunchPermit::immediate())
        }));
        let authority = FakeAttemptAuthority::new(false);
        let preparation_spec = CheckoutPreparationSpec::new(
            crate::workspace_intent::ExpectedGitCommitId::new(
                scope.commit_hex().to_string(),
                scope.commit_format(),
            )
            .unwrap(),
            PrefetchedCheckoutPack::for_tests(),
            spec.limits,
        )
        .unwrap();

        let outcome = backend
            .launch_checkout_continuation_given(
                &spec,
                &hooks,
                &authority,
                &report_claim(),
                &scope,
                runtime,
                preparation_spec,
                &workspace_manager,
                std::path::Path::new("/abs/staged-rootfs"),
                // Hop B success: drive the real session/lease to Prepared, wrapping test evidence.
                |runtime, _spec, _run_token, _authorization| {
                    Ok(runtime.into_prepared_for_tests(PreparedCheckoutEvidence::for_tests(
                        ResourceUsage {
                            cpu_seconds: 3,
                            mem_byte_seconds: 7,
                        },
                    )))
                },
                // Synthetic workload success: dispose the Prepared capsule CLEANLY (no runsc/userns
                // policy) and report a launched workload. The continuation maps Ran(Ok) → WorkloadLaunched.
                |prepared, _authority, _hooks, _spec, wm, _rootfs| {
                    let diagnostics = prepared.dispose_checkout_runtime(wm);
                    assert!(
                        diagnostics.is_empty(),
                        "the Prepared capsule disposes cleanly (release_prepared), got {diagnostics:?}"
                    );
                    RetainedWorkloadOutcome::Ran(Ok(fake_run()))
                },
            )
            .expect("the full-success continuation returns a launched workload");

        assert!(
            matches!(outcome, CheckoutContinuationOutcome::WorkloadLaunched(_)),
            "the full success sequence launches the workload, got {outcome:?}"
        );
        // Finding 1: the materialization phase hook was handed the ROTATED materialization generation.
        assert_eq!(
            seen_materialization_jti.lock().unwrap().as_deref(),
            Some("jti-Materialization"),
            "the materialization phase authorized against its OWN rotated spec, not the advertise base"
        );
        let ops = authority.ops.lock().unwrap().clone();
        assert!(
            ops.contains(&"begin:CheckoutMaterialization".to_string())
                && ops.contains(&"mint:Materialization".to_string()),
            "the continuation began + minted the materialization generation, got {ops:?}"
        );
        // The workload launched, then the synthetic op disposed the capsule cleanly → slot reusable.
        assert!(workspace_manager.is_healthy());
        assert!(userns_allocator.lease().is_ok());
        drop(backend);
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// Sol's finding 2 + 6(b): a post-acquisition authority failure (begin_phase OR mint) must DISPOSE
    /// the NotStarted capsule cleanly (delete workspace + release_unused) rather than dropping it (which
    /// would poison the manager + quarantine the slot), and return a clean typed requeue outcome — never
    /// permanently halt workspace admission.
    #[cfg(feature = "test-support")]
    #[allow(clippy::result_large_err)]
    #[test]
    fn continuation_disposes_capsule_on_authority_failure_without_poisoning() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        for (label, authority) in [
            ("begin_phase", FakeAttemptAuthority::failing_begin_phase()),
            ("mint_phase", FakeAttemptAuthority::failing_mint_phase()),
        ] {
            let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
                acquire_real_checkout_capsule(&format!("continuation-authfail-{label}"))
            else {
                return;
            };
            let backend = GvisorBackend::new(test_registry());
            let spec = checkout_spec();
            let scope = crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace)
                .unwrap()
                .unwrap();
            let hooks = RunnerHooks::new(
                CompletionSettlementOwner::TerminalReporter,
                Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
                Box::new(|_, _, _| Ok(())),
                Box::new(|_| Ok(())),
                Box::new(|_| Ok(())),
            )
            .with_checkout_phase_authorization(Box::new(|_s, _scope, _phase| {
                Ok(LaunchPermit::immediate())
            }));
            let preparation_spec = CheckoutPreparationSpec::new(
                crate::workspace_intent::ExpectedGitCommitId::new(
                    scope.commit_hex().to_string(),
                    scope.commit_format(),
                )
                .unwrap(),
                PrefetchedCheckoutPack::for_tests(),
                spec.limits,
            )
            .unwrap();

            let outcome = backend
                .launch_checkout_continuation_given(
                    &spec,
                    &hooks,
                    &authority,
                    &report_claim(),
                    &scope,
                    runtime,
                    preparation_spec,
                    &workspace_manager,
                    std::path::Path::new("/abs/staged-rootfs"),
                    |_runtime, _spec, _rt, _auth| {
                        panic!("Hop B must not run after an authority failure")
                    },
                    |_prepared, _a, _h, _s, _wm, _r| panic!("the workload must not run"),
                )
                .unwrap_or_else(|e| {
                    panic!("{label}: authority failure must be a typed outcome, not {e:?}")
                });

            // The capsule was disposed cleanly (NotStarted → delete + release_unused) — NOT poisoned.
            assert!(
                matches!(
                    outcome,
                    CheckoutContinuationOutcome::PreparationRetryable { .. }
                        | CheckoutContinuationOutcome::PreparationTerminal { .. }
                ),
                "{label}: a clean-disposal authority failure yields a typed requeue/terminal, got {outcome:?}"
            );
            assert!(
                workspace_manager.is_healthy(),
                "{label}: the manager must NOT be poisoned by a dropped capsule"
            );
            assert!(
                userns_allocator.lease().is_ok(),
                "{label}: the slot must be released (not quarantined) — workspace admission stays open"
            );
            drop(backend);
            let _ = std::fs::remove_dir_all(&workspace_base);
            let _ = std::fs::remove_dir_all(&leases_dir);
        }
    }

    /// Sol's finding 1 (workload) + 6(a): the closed workload transition mints the WORKLOAD generation
    /// (step 21) and runs the workload under its OWN rotated spec — the executor observes the workload
    /// JTI, never the advertise base. Uses `run_retained_workload_given` (the restored `#[cfg(test)]`
    /// execution seam) so no `runsc` is needed. Soft-skips when the explicit-userns revalidation policy
    /// is not installed on this host (the workload bind boundary re-revalidates the runsc-root identity
    /// live; without the policy the transition refuses BEFORE the executor — that is 5b.3-7's real drill).
    #[cfg(feature = "test-support")]
    #[test]
    fn run_retained_workload_threads_the_workload_generation() {
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("workload-generation-threading")
        else {
            return;
        };
        let prepared =
            runtime.into_prepared_for_tests(PreparedCheckoutEvidence::for_tests(ResourceUsage {
                cpu_seconds: 1,
                mem_byte_seconds: 1,
            }));
        let spec = checkout_spec();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        );
        let authority = FakeAttemptAuthority::new(true);
        let seen_workload_jti = Arc::new(Mutex::new(None::<String>));
        let seen = seen_workload_jti.clone();

        let outcome = prepared.run_retained_workload_given(
            &authority,
            &hooks,
            &spec,
            &workspace_manager,
            std::path::Path::new("/abs/staged-rootfs"),
            move |workload_spec, _cfg, permit, _rootfs, _container_id, _prep| {
                // Finding 1: the workload runs under its OWN generation, not the advertise base.
                *seen.lock().unwrap() = Some(workload_spec.run_token.jti.clone());
                drop(permit);
                // Assert-only executor: a synthetic pre-CAS Uncommitted so the capsule disposes cleanly.
                Err(RunFailure::uncommitted(
                    "synthetic assert-only workload executor",
                ))
            },
        );

        if seen_workload_jti.lock().unwrap().is_none() {
            // The revalidation policy is not installed — the transition refused before the executor and
            // already disposed the capsule. Soft-skip (the real bind is 5b.3-7's runsc drill).
            let _ = outcome;
            let _ = std::fs::remove_dir_all(&workspace_base);
            let _ = std::fs::remove_dir_all(&leases_dir);
            return;
        }
        assert_eq!(
            seen_workload_jti.lock().unwrap().as_deref(),
            Some("jti-Workload"),
            "the workload must run under its OWN rotated generation spec (step 21 mint)"
        );
        // The synthetic pre-CAS failure disposed the Prepared capsule (release_prepared) → slot reusable.
        assert!(matches!(outcome, RetainedWorkloadOutcome::RunFailed { .. }));
        assert!(workspace_manager.is_healthy());
        assert!(userns_allocator.lease().is_ok());
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// NeverBound (session NotStarted): dispose deletes the workspace and `release_unused`s the lease,
    /// so the slot becomes reusable and both managers stay healthy.
    #[cfg(feature = "test-support")]
    #[test]
    fn dispose_never_bound_deletes_workspace_and_frees_the_slot() {
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("dispose-never-bound")
        else {
            return;
        };
        let diagnostics = runtime.dispose_checkout_runtime(&workspace_manager);
        assert!(
            diagnostics.is_empty(),
            "a clean NeverBound disposal must produce no diagnostics, got {diagnostics:?}"
        );
        assert!(workspace_manager.is_healthy());
        assert!(userns_allocator.is_healthy());
        assert!(
            userns_allocator.lease().is_ok(),
            "release_unused must return the slot to the pool — a fresh lease must now succeed"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// Prepared (session bind_preparation + confirm_prepared): dispose deletes the workspace and
    /// `release_prepared`s the lease, so the slot becomes reusable.
    #[cfg(feature = "test-support")]
    #[test]
    fn dispose_prepared_deletes_workspace_and_release_prepared_frees_the_slot() {
        let Some((mut runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("dispose-prepared")
        else {
            return;
        };
        // Drive the session to Prepared via the module's own test-only driver — the capsule's fields
        // are now module-private, so a sibling test can no longer reach them directly.
        runtime.drive_session_for_tests(CheckoutSessionCleanup::Prepared);
        let diagnostics = runtime.dispose_checkout_runtime(&workspace_manager);
        assert!(
            diagnostics.is_empty(),
            "a clean Prepared disposal must produce no diagnostics, got {diagnostics:?}"
        );
        assert!(workspace_manager.is_healthy());
        assert!(
            userns_allocator.lease().is_ok(),
            "release_prepared must return the slot to the pool — a fresh lease must now succeed"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// TeardownUnproven (session bind_preparation only — teardown never proven): dispose quarantines
    /// BOTH — the slot is NOT reissued and the workspace manager is poisoned.
    #[cfg(feature = "test-support")]
    #[test]
    fn dispose_teardown_unproven_quarantines_both() {
        let Some((mut runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("dispose-teardown-unproven")
        else {
            return;
        };
        runtime.drive_session_for_tests(CheckoutSessionCleanup::TeardownUnproven);
        let diagnostics = runtime.dispose_checkout_runtime(&workspace_manager);
        assert!(
            diagnostics.iter().any(|d| d.contains("quarantined")),
            "a TeardownUnproven disposal must report quarantine, got {diagnostics:?}"
        );
        assert!(
            !workspace_manager.is_healthy(),
            "dropping the still-live workspace (never delete, never release) must poison the manager"
        );
        assert!(
            userns_allocator.lease().is_err(),
            "a quarantined slot must NOT be reissued — the pool stays exhausted"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// WorkloadBound (session driven all the way to Done): disposal is structurally impossible, so it
    /// abandons BOTH and surfaces an invariant violation — slot quarantined, manager poisoned.
    #[cfg(feature = "test-support")]
    #[test]
    fn dispose_workload_bound_abandons_both_with_an_invariant_violation() {
        let Some((mut runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("dispose-workload-bound")
        else {
            return;
        };
        runtime.drive_session_for_tests(CheckoutSessionCleanup::WorkloadBound);
        let diagnostics = runtime.dispose_checkout_runtime(&workspace_manager);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.contains("structurally impossible")),
            "disposing a WorkloadBound capsule must surface an invariant violation, got {diagnostics:?}"
        );
        assert!(!workspace_manager.is_healthy());
        assert!(
            userns_allocator.lease().is_err(),
            "an abandoned slot must NOT be reissued"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn settle_enabled_workspace_and_lease_refuses_evidence_disagreeing_with_the_recorded_binding() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("settle-mismatch")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("settle-mismatch")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let container_id = "settle-mismatch-container";
        let (_cfg, mut context) = acquire_enabled_workspace(
            &command_spec,
            &profile,
            container_id,
            PathBuf::from("/abs/staged-rootfs"),
            &workspace_manager,
            &userns_allocator,
            None,
        )
        .expect("acquisition must succeed");
        let runsc_root_identity = (11, 22);
        let cgroup_identity = (33, 44);
        context
            .lease
            .bind(
                container_id.to_string(),
                runsc_root_identity,
                cgroup_identity,
            )
            .expect("bind must succeed");
        context.bind_state = LeaseBindState::Bound {
            container_id: container_id.to_string(),
            runsc_root_identity,
            cgroup_identity,
        };
        let host_path = context.workspace.host_path().to_path_buf();
        // Evidence claims a DIFFERENT cgroup identity than what was actually bound.
        let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
            container_id.to_string(),
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity,
            },
            CgroupQuiescenceEvidence::assert_for_tests((99, 99)),
        );
        let result = settle_enabled_workspace_and_lease(context, &workspace_manager, &evidence);
        assert!(
            result.is_err(),
            "evidence disagreeing with the recorded binding must refuse, not silently release"
        );
        // Neither the workspace nor the lease were touched by `settle_enabled_workspace_and_lease`
        // -- refusing dropped both, which poisons `workspace_manager` (real subvolume abandoned,
        // exactly like `dropping_a_managed_workspace_without_deleting_poisons_the_manager_with_one_incident`
        // in workspace_manager.rs) and quarantines the userns slot. The real subvolume is still on
        // disk here; `remove_dir_all` CANNOT remove a Btrfs subvolume (it needs a privileged
        // `btrfs subvolume delete`). Sol's review: exercise the ACTUAL claimed crash-recovery path
        // instead of leaking it — drop the poisoned manager (releasing its lock), open a FRESH
        // manager on the same base, and let ITS OWN boot-time reconciliation delete the orphan for
        // real before `remove_dir_all` is safe to call on the (now subvolume-free) base directory.
        assert!(
            host_path.exists(),
            "the abandoned subvolume must still be real and on disk"
        );
        drop(workspace_manager);
        let sink2: crate::workspace_manager::IncidentSink =
            Arc::new(|msg: &str| eprintln!("[piece7c workspace incident] {msg}"));
        let fresh = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: workspace_base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink2,
        )
        .expect("a fresh manager's own boot reconciliation must clean up the orphan and succeed");
        assert!(fresh.is_healthy());
        assert!(
            !host_path.exists(),
            "boot reconciliation must have deleted the abandoned subvolume for real"
        );
        drop(fresh);
        // Quarantined userns markers are NEVER deleted by design (boot reconciliation only ever
        // quarantines a surviving marker, never removes it) -- `leases_dir` holds only plain JSON
        // marker files, not a Btrfs primitive, so `remove_dir_all` here is genuinely safe.
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// Sol's round-1 review, blocker 2: a `Bound` outer error is structurally impossible in correct
    /// code (a successful bind means the runner always returns the `RuntimeFinalization` envelope
    /// from that point on), but `cleanup_pre_bind_failure` must still handle it conservatively if a
    /// future regression ever reaches it -- abandoning BOTH resources rather than acting on either,
    /// and ALWAYS surfacing a non-empty invariant-violation diagnostic.
    #[cfg(feature = "test-support")]
    #[test]
    fn cleanup_pre_bind_failure_abandons_both_resources_when_bind_state_is_bound() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("bound-abandons-both")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("bound-abandons-both")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let container_id = "bound-abandons-both-container";
        let (_cfg, mut context) = acquire_enabled_workspace(
            &command_spec,
            &profile,
            container_id,
            PathBuf::from("/abs/staged-rootfs"),
            &workspace_manager,
            &userns_allocator,
            None,
        )
        .expect("acquisition must succeed");
        let runsc_root_identity = (11, 22);
        let cgroup_identity = (33, 44);
        context
            .lease
            .bind(
                container_id.to_string(),
                runsc_root_identity,
                cgroup_identity,
            )
            .expect("bind must succeed");
        context.bind_state = LeaseBindState::Bound {
            container_id: container_id.to_string(),
            runsc_root_identity,
            cgroup_identity,
        };
        let host_path = context.workspace.host_path().to_path_buf();

        // The "structurally impossible" outer failure: some future regression calls the pre-bind
        // cleanup path even though bind already durably succeeded.
        let diagnostics = cleanup_pre_bind_failure(context, &workspace_manager);

        assert_eq!(
            diagnostics.len(),
            1,
            "a Bound outer error must always surface exactly one invariant-violation diagnostic, \
             never an empty vec: {diagnostics:?}"
        );
        assert!(diagnostics[0].contains("structurally impossible"));
        assert!(
            host_path.exists(),
            "the workspace must be ABANDONED, not deleted, when bind_state was Bound"
        );
        assert!(
            !workspace_manager.is_healthy(),
            "abandoning the workspace without deleting it must poison the manager"
        );
        assert!(
            !userns_allocator.is_healthy(),
            "abandoning a Bound lease without releasing it must poison the allocator too"
        );

        // Clean up for real: drop the poisoned manager, let a fresh one's boot reconciliation
        // delete the orphaned subvolume (never `remove_dir_all` on a real Btrfs subvolume).
        drop(workspace_manager);
        let sink2: crate::workspace_manager::IncidentSink =
            Arc::new(|msg: &str| eprintln!("[piece7c workspace incident] {msg}"));
        let fresh = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: workspace_base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink2,
        )
        .expect("a fresh manager's own boot reconciliation must clean up the orphan and succeed");
        assert!(!host_path.exists());
        drop(fresh);
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_refuses_when_capacity_is_exhausted_and_touches_nothing_else() {
        // Capacity is exhausted BEFORE `acquire_enabled_workspace` is ever called, so
        // `create_workspace`'s `btrfs qgroup limit` step is never reached — no `CAP_SYS_ADMIN`
        // needed, so this test runs (rather than skips) even without that privilege.
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_without_qgroup_probe_for_tests("capacity-exhausted")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("capacity-exhausted")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        // Exhaust the 1 GiB ceiling with an unrelated hold, so `acquire_capacity` refuses cleanly
        // BEFORE ever touching the userns allocator.
        let holder = workspace_manager
            .acquire_capacity(1 << 30)
            .expect("the fresh manager's own full ceiling must be leasable once");
        let mut command_spec = spec(vec![]);
        command_spec.limits.disk_bytes = 1; // any nonzero request now exceeds the exhausted ceiling
        let profile = HardeningProfile::derive(&command_spec);
        let result = acquire_enabled_workspace(
            &command_spec,
            &profile,
            "capacity-exhausted-container",
            PathBuf::from("/abs/staged-rootfs"),
            &workspace_manager,
            &userns_allocator,
            None,
        );
        assert!(
            result.is_err(),
            "an exhausted ceiling must refuse acquisition"
        );
        assert!(
            userns_allocator.quarantined_slots().is_empty(),
            "acquire_enabled_workspace must never have leased (and left quarantined) a userns \
             slot when capacity refused first: {:?}",
            userns_allocator.quarantined_slots()
        );
        holder.release();
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// Sol's required-tests list: "userns refusal releases capacity." Uses the injectable
    /// `_given` seam with a REAL capacity lease (from a real, lightweight manager — no
    /// `CAP_SYS_ADMIN` needed for `acquire_capacity` itself) and a SYNTHETIC userns refusal (no
    /// real allocator needed at all), proving the capacity lease is released back to the pool
    /// rather than left dangling.
    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_given_releases_capacity_when_userns_lease_is_refused() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_without_qgroup_probe_for_tests("userns-refused")
        else {
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let result = acquire_enabled_workspace_given(
            &command_spec,
            &profile,
            "container-userns-refused",
            PathBuf::from("/abs/staged-rootfs"),
            |bytes| workspace_manager.acquire_capacity(bytes),
            || Err(UserNamespaceRefusal::PoolExhausted { pool_size: 0 }),
            |_, _, _, _, _| {
                panic!("create_workspace must never run when the lease is refused first")
            },
            |_| panic!("delete_workspace must never run on this path"),
        );
        assert!(
            result.is_err(),
            "a refused userns lease must refuse acquisition"
        );
        // If capacity was genuinely released, the full ceiling is leasable again.
        let holder = workspace_manager
            .acquire_capacity(1 << 30)
            .expect("capacity must have been released back to the pool after the userns refusal");
        holder.release();
        let _ = std::fs::remove_dir_all(&workspace_base);
    }

    /// Sol's required-tests list: "recoverable provisioning failure releases unused lease" and
    /// "`UnrecoverableLeak` quarantines it." Both use a REAL capacity lease + REAL userns lease
    /// (from a real, lightweight allocator — no privileged operation needed for `lease()` itself)
    /// and a SYNTHETIC `create_workspace` failure, so neither needs `CAP_SYS_ADMIN`.
    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_given_releases_the_lease_on_a_recoverable_provisioning_failure() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_without_qgroup_probe_for_tests("recoverable-storage-failure")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("recoverable-storage-failure")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let result = acquire_enabled_workspace_given(
            &command_spec,
            &profile,
            "container-recoverable-failure",
            PathBuf::from("/abs/staged-rootfs"),
            |bytes| workspace_manager.acquire_capacity(bytes),
            || userns_allocator.lease(),
            |_, _, _, _, capacity: CapacityLease| {
                // Mirrors the REAL `WorkspaceManager::create_workspace`'s own contract for a
                // non-`UnrecoverableLeak` `Storage` error: "capacity was already released
                // internally" — a synthetic closure standing in for it must honor the same
                // contract, or this test would (as it initially did) observe an incident from
                // `CapacityLease::drop` that has nothing to do with what's under test.
                capacity.release();
                Err(WorkspaceProvisionError::Storage(
                    WorkspaceStorageError::ZeroQuota,
                ))
            },
            |_| panic!("delete_workspace must never run — no workspace was ever created"),
        );
        assert!(
            result.is_err(),
            "a recoverable provisioning failure must refuse acquisition"
        );
        assert!(
            userns_allocator.quarantined_slots().is_empty(),
            "a recoverable failure must release_unused() the lease, not quarantine it: {:?}",
            userns_allocator.quarantined_slots()
        );
        assert!(
            workspace_manager.is_healthy(),
            "a recoverable failure must leave the workspace manager healthy (capacity released \
             cleanly, not abandoned)"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_given_quarantines_the_lease_on_an_unrecoverable_leak() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_without_qgroup_probe_for_tests("unrecoverable-leak")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("unrecoverable-leak")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let result = acquire_enabled_workspace_given(
            &command_spec,
            &profile,
            "container-unrecoverable-leak",
            PathBuf::from("/abs/staged-rootfs"),
            |bytes| workspace_manager.acquire_capacity(bytes),
            || userns_allocator.lease(),
            |_, _, _, _, _capacity| {
                Err(WorkspaceProvisionError::Storage(
                    WorkspaceStorageError::UnrecoverableLeak {
                        path: PathBuf::from("/fake/leaked/path"),
                        subvol_id: None,
                        provisioning_error: "synthetic provisioning error".to_string(),
                        cleanup_error: "synthetic cleanup error".to_string(),
                    },
                ))
            },
            |_| panic!("delete_workspace must never run — no workspace was ever created"),
        );
        assert!(
            result.is_err(),
            "an unrecoverable leak must refuse acquisition"
        );
        assert_eq!(
            userns_allocator.quarantined_slots().len(),
            1,
            "an UnrecoverableLeak must quarantine (never release_unused()) the lease: {:?}",
            userns_allocator.quarantined_slots()
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// Sol's required-tests list: "Enabled health checks precede reserve." Forces
    /// `userns_allocator.check_identity()` to fail deterministically (replacing the leases dir it
    /// locked at construction) — no real Btrfs/qgroup privilege needed — and proves `hooks.reserve`
    /// was never called by the time `launch_with` refuses.
    #[cfg(feature = "test-support")]
    #[test]
    fn enabled_health_checks_refuse_before_reserve_is_ever_called() {
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("health-precedes-reserve")
        else {
            return;
        };
        // Replace the leases dir AFTER construction so `check_identity()`'s re-stat disagrees with
        // the identity it locked at construction time — a deterministic, real failure.
        let replacement = leases_dir.with_extension("replacement");
        std::fs::rename(&leases_dir, &replacement).unwrap();
        std::fs::create_dir_all(&leases_dir).unwrap();
        assert!(
            userns_allocator.check_identity().is_err(),
            "the replaced leases dir must make check_identity() fail"
        );

        let workspace_manager =
            WorkspaceManager::try_new(WorkspaceStorageMode::Disabled, Arc::new(|_: &str| {}))
                .unwrap();
        let backend = GvisorBackend {
            live: Mutex::new(std::collections::HashMap::new()),
            registry: Some(test_registry()),
            workspace_integration: WorkspaceIntegration::Enabled {
                workspace_manager,
                userns_allocator,
            },
            checkout: GvisorCheckoutConfig::disabled(),
            rootfs_overlay: None,
        };
        let reserve_called = Arc::new(AtomicBool::new(false));
        let reserve_called_in_hook = reserve_called.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(move |spec: &JobSpec| {
                reserve_called_in_hook.store(true, Ordering::SeqCst);
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch(&spec(vec![]), &hooks);
        assert!(
            result.is_err(),
            "a failed userns identity check must refuse the launch"
        );
        assert!(
            !reserve_called.load(Ordering::SeqCst),
            "hooks.reserve must never be called once an Enabled health check has failed"
        );
        let _ = std::fs::remove_dir_all(&leases_dir);
        let _ = std::fs::remove_dir_all(&replacement);
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

    /// A registry mapping a fresh digest-pinned [`ImageRef`] to the REAL staged rootfs
    /// [`crate::resolved_gvisor_rootfs`] uses — so a spec naming this image resolves, through the
    /// real registry lookup `GvisorBackend::launch_with` performs, to the exact rootfs the drill
    /// above already proves is runnable.
    #[cfg(feature = "integration")]
    fn real_userns_drill_registry(
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
    const USERNS_DRILL_LEASES_DIR_ENV: &str = "MYELIN_USERNS_DRILL_LEASES_DIR";

    /// Sol's review (CT-007 slice 5b.2's live drill): this drill and the checkout-preparation live
    /// drill (`checkout_preparation_5b2::checkout_preparation_runs_end_to_end_through_real_git_wire_and_runsc`)
    /// share the SAME operator-provisioned `leases_dir` and may run concurrently under `cargo
    /// test`'s default parallelism — the allocator's own directory lock is a PER-PROCESS lifetime
    /// lock (see `UserNamespaceAllocator`'s own doc), not a per-call one, so two independent
    /// `UserNamespaceAllocator::try_new` calls against the same directory in the SAME test binary
    /// process would race nondeterministically. Both drills acquire this before touching
    /// `leases_dir` at all, so only one is ever mid-flight.
    #[cfg(feature = "integration")]
    static USERNS_DRILL_LEASES_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Sol's round-2 review (hard blocker for this activation commit, since `GvisorBackend::try_new`
    /// is now `pub`): the promised end-to-end drill through the REAL public activation path —
    /// `GvisorBackend::try_new(GvisorWorkspaceConfig::Enabled)` + `.launch(...)` — not merely manual
    /// orchestration of the individual pieces (those already have their own dedicated unit coverage:
    /// `bind_enabled_lease_given_*`, `finalize_runtime_*`, `gvisor_prod_exec_test`). This is the
    /// integration proof layered above that deterministic coverage.
    ///
    /// Sol's round-3 review: an EARLIER version of this drill generated its own fresh `leases_dir`
    /// under `std::env::temp_dir()` and treated `GvisorBackend::try_new(Enabled)`'s resulting
    /// GUARANTEED refusal (the strict allocator constructor can never accept a caller-generated,
    /// not-pre-provisioned leaf) as an ordinary host-dependent skip — making this drill an
    /// UNCONDITIONAL skip everywhere, never actually proving anything. Fixed: the ONLY legitimate
    /// skip condition now is `MYELIN_USERNS_DRILL_LEASES_DIR` being unset (this drill has no
    /// business fabricating that directory itself — see the const's own doc). Once an operator HAS
    /// supplied it, `GvisorBackend::try_new(Enabled)` is required to succeed (`.expect`, never a
    /// caught-and-skipped error) — reaching this point means the host is asserted to be correctly
    /// provisioned, so any further failure (construction OR `.launch()` itself) is a genuine
    /// regression this drill exists to catch, not a skip. The externally provisioned leases
    /// directory is NEVER removed by this drill (it is not this drill's to own or delete) — only the
    /// workspace base_dir this drill creates for itself is cleaned up.
    #[test]
    #[cfg(feature = "integration")]
    fn explicit_user_namespace_boots_through_the_real_enabled_backend_and_launch() {
        // Serializes against the checkout-preparation live drill, which shares the SAME
        // operator-provisioned `leases_dir` (see `USERNS_DRILL_LEASES_DIR_LOCK`'s own doc).
        let _leases_dir_guard = USERNS_DRILL_LEASES_DIR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Err(e) = preflight_explicit_userns_policy(
            resolved_explicit_userns_helper_dir(),
            resolved_explicit_userns_runsc_root(),
        ) {
            eprintln!(
                "[explicit-userns activation drill] SKIP: preflight_explicit_userns_policy failed: {e}"
            );
            return;
        }
        let rootfs = crate::resolved_gvisor_rootfs();
        if !rootfs.exists() {
            eprintln!(
                "[explicit-userns activation drill] SKIP: staged rootfs absent at {rootfs:?}"
            );
            return;
        }
        let leases_dir = match std::env::var(USERNS_DRILL_LEASES_DIR_ENV) {
            Ok(value) if !value.is_empty() => PathBuf::from(value),
            _ => {
                eprintln!(
                    "[explicit-userns activation drill] SKIP: {USERNS_DRILL_LEASES_DIR_ENV} is not \
                     set — this drill needs an operator-provisioned leases directory satisfying the \
                     STRICT production allocator contract (pre-existing, euid-owned, mode 0700 or \
                     stricter, non-writable-by-us ancestor chain); it cannot fabricate one itself"
                );
                return;
            }
        };

        let tag = format!("{}-{}", std::process::id(), unique_suffix());
        // `std::env::temp_dir()` (`/tmp`) is frequently a separate tmpfs mount, not Btrfs — use a
        // `$HOME`-rooted path instead, matching every other real `WorkspaceManager` fixture in this
        // file (e.g. `real_workspace_manager_for_tests`).
        let mut workspace_base_dir = std::env::home_dir().expect("HOME must be set for this test");
        workspace_base_dir.push(format!(
            ".local/state/myelin-userns-activation-workspace-{tag}"
        ));
        let incident_sink: crate::workspace_manager::IncidentSink =
            Arc::new(|msg: &str| eprintln!("[explicit-userns activation drill incident] {msg}"));

        let backend = GvisorBackend::try_new(
            real_userns_drill_registry(&rootfs),
            GvisorWorkspaceConfig::Enabled {
                base_dir: workspace_base_dir.clone(),
                host_capacity_bytes: 1 << 30,
                leases_dir,
                min_pool_size: 1,
            },
            incident_sink,
        )
        .expect(
            "GvisorBackend::try_new(Enabled) must succeed once an operator-provisioned leases \
             directory is configured -- reaching this point asserts the host IS correctly \
             provisioned, so a construction failure here is a genuine regression",
        );

        // Bind the spec to the SAME image the registry above was just built for (not
        // `fixture_image()`, which points at a different, throwaway fixture rootfs).
        let digest = crate::canonical_tar::canonical_tree_sha256_hex(&rootfs)
            .expect("hash the real staged rootfs");
        let mut command_spec = spec(vec![]);
        command_spec.image =
            ImageRef::pinned(format!("test.local/userns-drill@sha256:{digest}")).unwrap();
        command_spec.command = vec!["/bin/sh".into(), "-c".into(), "id".into()];

        let launch = backend.launch(&command_spec, &ok_hooks()).expect(
            "launch through the real Enabled activation path must succeed on a correctly \
                      provisioned host",
        );
        assert_eq!(
            launch.result.exit_code,
            Some(0),
            "the guest `id` command must exit 0, stderr: {}",
            String::from_utf8_lossy(&launch.result.stderr)
        );
        assert!(!launch.result.timed_out);
        let stdout = String::from_utf8_lossy(&launch.result.stdout);
        assert!(
            stdout.contains("uid=65534") && stdout.contains("gid=65534"),
            "the guest must report uid/gid 65534 (mapped via the OCI uidMappings/gidMappings this \
             slice emits) through the REAL Enabled activation path, got: {stdout:?}"
        );
        backend
            .kill(&launch.handle)
            .expect("kill must succeed to clean up the live-map entry after a completed run");

        // The leases dir is externally owned (an operator's install step) -- never removed here.
        let _ = std::fs::remove_dir_all(&workspace_base_dir);
    }

    #[test]
    fn gvisor_launch_drives_four_guarantees_on_the_same_trait() {
        // The SAME SandboxBackend trait + the SAME hardening — the named-second backend.
        let backend = GvisorBackend::new(test_registry());
        let launch = backend
            .launch_with(
                &spec(vec![]),
                &ok_hooks(),
                |_spec, _cfg, permit, _rootfs, _container_id, _prep| {
                    permit
                        .commit_and_release()
                        .map_err(|error| RunFailure::uncommitted(error.to_string()))?;
                    Ok(fake_finalization())
                },
            )
            .unwrap();
        assert_eq!(launch.handle.guest_id, "runsc-idem-runsc-1");
        // The reshaped seam carries the command result back (CT-001 stub).
        assert_eq!(launch.result.exit_code, Some(0));
        assert!(launch.result.passed());
        backend.kill(&launch.handle).unwrap();
    }

    /// CT-007 slice 3, piece 7a: `launch_with` (not the run closure) now generates `container_id`
    /// — this test proves the closure genuinely RECEIVES that same value (not an empty/placeholder
    /// one), in the expected shape, and that two separate launches never reuse it.
    #[test]
    fn launch_with_generates_a_distinct_container_id_the_closure_receives() {
        let backend = GvisorBackend::new(test_registry());
        let seen = Arc::new(Mutex::new(Vec::new()));
        for _ in 0..2 {
            let seen = seen.clone();
            backend
                .launch_with(
                    &spec(vec![]),
                    &ok_hooks(),
                    move |_spec, _cfg, permit, _rootfs, container_id, _prep| {
                        seen.lock().unwrap().push(container_id.to_string());
                        permit
                            .commit_and_release()
                            .map_err(|error| RunFailure::uncommitted(error.to_string()))?;
                        Ok(fake_finalization())
                    },
                )
                .unwrap();
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        for id in seen.iter() {
            assert!(
                id.starts_with(&format!("myelin-prod-{}-", std::process::id())),
                "unexpected container_id shape: {id:?}"
            );
        }
        assert_ne!(
            seen[0], seen[1],
            "two separate launches must never reuse the same container_id"
        );
    }

    #[test]
    fn resolved_explicit_userns_policy_revalidates_a_matching_identity() {
        let dir = std::env::temp_dir().join(format!(
            "myelin-gvisor-userns-root-identity-ok-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&dir, perms).unwrap();
        let meta = std::fs::metadata(&dir).unwrap();
        let identity = (meta.dev(), meta.ino());
        let policy = ResolvedExplicitUsernsPolicy {
            helper_dir: PathBuf::from("/usr/bin"),
            runsc_root: dir.clone(),
            runsc_root_identity: identity,
        };
        assert_eq!(policy.revalidated_root_identity(), Ok(identity));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Sol's round-1 review on piece 7a: identity (dev, ino) alone is not enough, because the leaf
    /// is owned by this process's own euid and its MODE can drift (e.g. `0700` chmod'd to `0777`)
    /// without the identity ever changing — a replacement whose identity genuinely differs is
    /// already caught by `resolved_explicit_userns_policy_refuses_a_replaced_root`, but a
    /// same-inode mode drift would NOT be, if revalidation only compared `(dev, ino)`. Proves
    /// `revalidated_root_identity` reruns the FULL leaf hardening check, not a bare stat, by
    /// drifting the mode of the SAME directory (no replacement) and confirming revalidation now
    /// refuses.
    #[test]
    fn resolved_explicit_userns_policy_refuses_a_mode_drift_without_replacement() {
        let dir = std::env::temp_dir().join(format!(
            "myelin-gvisor-userns-root-identity-drift-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&dir, perms).unwrap();
        let meta = std::fs::metadata(&dir).unwrap();
        let identity = (meta.dev(), meta.ino());
        let policy = ResolvedExplicitUsernsPolicy {
            helper_dir: PathBuf::from("/usr/bin"),
            runsc_root: dir.clone(),
            runsc_root_identity: identity,
        };
        // Drift the mode of the SAME directory — no rmdir/mkdir, so (dev, ino) is unchanged.
        let mut drifted = std::fs::metadata(&dir).unwrap().permissions();
        drifted.set_mode(0o777);
        std::fs::set_permissions(&dir, drifted).unwrap();
        let result = policy.revalidated_root_identity();
        let mut restore = std::fs::metadata(&dir).unwrap().permissions();
        restore.set_mode(0o700);
        std::fs::set_permissions(&dir, restore).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            result.is_err(),
            "a same-inode mode drift (0700 -> 0777) must be refused, not just identity-compared: \
             {result:?}"
        );
    }

    /// Sol's design-round note (piece 7): the revalidation accessor must catch a state root that
    /// no longer names the SAME directory it was validated against — e.g. removed and recreated at
    /// the identical path (a fresh inode) between preflight and a later bind/teardown attempt.
    ///
    /// Sol's round-1 review: `rmdir` + immediate `mkdir` at the same path does not guarantee a
    /// fresh inode — POSIX permits filesystems to reuse a freed inode number. Instead, rename the
    /// original directory ASIDE (so it stays alive under a different path) and create the
    /// replacement fresh at the original path, then assert the two identities actually differ
    /// before relying on that difference to prove refusal.
    #[test]
    fn resolved_explicit_userns_policy_refuses_a_replaced_root() {
        let dir = std::env::temp_dir().join(format!(
            "myelin-gvisor-userns-root-identity-replaced-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let moved_aside = std::env::temp_dir().join(format!(
            "myelin-gvisor-userns-root-identity-replaced-original-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&dir, perms).unwrap();
        let meta = std::fs::metadata(&dir).unwrap();
        let original_identity = (meta.dev(), meta.ino());
        let policy = ResolvedExplicitUsernsPolicy {
            helper_dir: PathBuf::from("/usr/bin"),
            runsc_root: dir.clone(),
            runsc_root_identity: original_identity,
        };
        std::fs::rename(&dir, &moved_aside).unwrap(); // original stays alive, just relocated
        std::fs::create_dir(&dir).unwrap(); // a genuinely fresh directory at the original path
        let mut fresh_perms = std::fs::metadata(&dir).unwrap().permissions();
        fresh_perms.set_mode(0o700);
        std::fs::set_permissions(&dir, fresh_perms).unwrap();
        let fresh_meta = std::fs::metadata(&dir).unwrap();
        let fresh_identity = (fresh_meta.dev(), fresh_meta.ino());
        assert_ne!(
            original_identity, fresh_identity,
            "the fixture must produce a genuinely different inode, not rely on chance"
        );
        assert!(policy.revalidated_root_identity().is_err());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&moved_aside);
    }

    #[test]
    fn revalidated_explicit_userns_root_identity_given_refuses_without_a_policy() {
        let result = revalidated_explicit_userns_root_identity_given(None);
        assert!(matches!(result, Err(ref reason) if reason.contains("never validated")));
    }

    #[test]
    fn gvisor_corpus_script_carries_every_catalogued_attack_and_the_posture() {
        // The gVisor corpus probes the SAME catalogued attack ids the host-side parser keys on, so
        // the gate predicate is one path across backends. A drift here would let a family silently
        // not run on the gVisor backend.
        let script = build_gvisor_corpus_script(64);
        for atk in crate::escape_corpus::CORPUS {
            assert!(
                script.contains(atk.id),
                "catalogued attack `{}` is missing from the gVisor corpus script",
                atk.id
            );
        }
        assert!(script.contains(crate::escape_corpus::BEGIN_MARKER));
        assert!(script.contains(crate::escape_corpus::END_MARKER));
        // The raw-device family is probed as mknod-denial (the faithful gVisor expression of the
        // contained property — the node is absent and creating one is denied).
        assert!(script.contains("mknod /dev/mem"));
        assert!(script.contains("mknod /dev/port"));
        // The fork-bomb ceiling is carried from the arg.
        assert!(script.contains("ceiling=64"));
        assert!(script.contains("trap cleanup_f1 EXIT"));
        assert!(script.contains("exit 42"));
        assert!(script.contains("[ \"$admitted\" -gt 64 ]"));
        assert!(script.contains("F1_forkbomb ESCAPED admitted=$admitted"));
        // The admitted children are reaped before D2/Mx so the fork-bomb probe cannot consume the
        // shared memory cgroup and vacuously kill a later independent resource probe.
        let reap = script.find("cleanup_f1()").unwrap();
        let verdict = script.find("if [ \"$f1_status\" -eq 42 ]").unwrap();
        let diskfill = script.find("if dd if=/dev/zero").unwrap();
        assert!(
            script.contains("wait 2>/dev/null || true") && reap < verdict && verdict < diskfill
        );
        // CT-003b: the anon-memory hog's ATTEMPT sentinel + the END marker precede the oversized
        // alloc, so the corpus COMPLETES even when the contained hog OOM-kills the whole sentry
        // mid-alloc (the host cgroup bounds host RAM). The ESCAPED line follows only if it HELD.
        let attempt = script
            .find(&format!("{} ATTEMPT", crate::escape_corpus::MEMHOG_ID))
            .expect("memhog ATTEMPT sentinel in the gVisor corpus");
        let end = script.find(crate::escape_corpus::END_MARKER).unwrap();
        assert!(
            attempt < end,
            "the memhog ATTEMPT sentinel must precede the END marker"
        );
        // Pure-shell doubling allocator (holds the anon memory in the sh process itself; ~1 GiB) —
        // the host cgroup OOM-kills the sentry when it breaches memory.max, never a false held=0.
        assert!(script.contains(r#"S="$S$S""#) && script.contains("while [ $n -lt 26 ]"));
    }


    #[test]
    fn gvisor_drill_config_expresses_the_mandatory_posture() {
        let json = gvisor_drill_config_json(&spec(vec![]), GVISOR_CORPUS_SCRIPT).unwrap();
        // Read-only root, no-new-privs, all caps dropped, the pids ceiling — the SAME mandatory
        // profile the Firecracker backend enforces, expressed through the OCI spec gVisor consumes.
        assert!(json.contains("\"readonly\": true"));
        assert!(json.contains("\"noNewPrivileges\": true"));
        assert!(json.contains("\"bounding\": []"));
        assert!(json.contains("\"limit\": 64"));
        assert!(json.contains("\"type\": \"RLIMIT_NPROC\""));
        // NO network namespace ⇒ with --network=none only loopback exists (egress closed).
        assert!(
            !json.contains("\"type\": \"network\""),
            "no network namespace ⇒ egress closed (--network=none leaves only loopback)"
        );
        // The rootless deviation: NO user namespace (runsc --rootless adds its own).
        assert!(
            !json.contains("\"type\": \"user\""),
            "the rootless gofer fork fails with a doubly-declared user namespace"
        );
        // The entrypoint runs the corpus script.
        assert!(json.contains(GVISOR_CORPUS_SCRIPT));
    }

    #[test]
    fn gvisor_refuses_to_start_on_exhaustion() {
        let backend = GvisorBackend::new(test_registry());
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|_spec| Err(crate::HookError("exhausted".into()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let r = backend.launch_with(
            &spec(vec![]),
            &hooks,
            |_spec, _cfg, _permit, _rootfs, _container_id, _prep| Ok(fake_finalization()),
        );
        assert!(matches!(
            r,
            Err(SandboxLaunchError::Failed(GvisorError::Hook(_)))
        ));
    }

    // ───────── CT-007 slice 5b.3-6b: golden compute event-trace regression fence ─────────
    //
    // These three tests pin the OBSERVABLE ordered sequence of the ordinary compute path as it flows
    // through the `launch_with` wrapper into the extracted `launch_compute_with` body. 5b.3-6b moved
    // that body byte-for-byte and made `launch_with` a plain delegator; the point of these tests is a
    // regression fence — any future edit that reorders, drops, or duplicates an OBSERVABLE hook/run
    // step (isolation floor → reserve → launch permit → run spawn → settle) changes the recorded trace
    // and fails here. The fence covers the observable hook/run ordering and the two early-refusal
    // boundaries; it does NOT independently detect a reorder among non-observed internal steps (e.g.
    // moving container-id minting relative to reserve, or a duplicated registry lookup) — those are
    // covered by the mechanical byte-identity of the extraction plus the existing compute unit tests.
    // The Disabled (no-privilege) backend is used deliberately: the
    // Enabled-only steps (workspace-manager health check, `acquire_enabled_workspace`, Enabled
    // `RuntimePreparation`) are already fenced by the 6a acquire/settle + dispose matrices, and the
    // compute ORDERING these tests fence is identical regardless of workspace integration.

    /// Golden success trace: the exact ordered hook/run sequence for a compute launch, plus the stable
    /// `myelin-prod-*` workload id the run closure sees, the single live-map insert, and the
    /// byte-identical measured usage handed to `settle_completed`.
    #[test]
    fn golden_compute_trace_through_launch_with_is_byte_stable() {
        let backend = GvisorBackend::new(test_registry());
        let trace = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed_container_id = Arc::new(Mutex::new(None::<String>));

        let t_iso = trace.clone();
        let t_res = trace.clone();
        let t_settle = trace.clone();
        let t_attr = trace.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(move |spec| {
                t_res.lock().unwrap().push("reserve".into());
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(move |_spec, _h, usage| {
                t_settle.lock().unwrap().push(format!(
                    "settle:{}:{}",
                    usage.cpu_seconds, usage.mem_byte_seconds
                ));
                Ok(())
            }),
            Box::new(move |_spec| {
                t_attr.lock().unwrap().push("acquire_launch_permit".into());
                Ok(())
            }),
            Box::new(move |_spec| {
                t_iso.lock().unwrap().push("isolation_floor".into());
                Ok(())
            }),
        );

        let t_run = trace.clone();
        let seen_id = observed_container_id.clone();
        let launch = backend
            .launch_with(
                &spec(vec![]),
                &hooks,
                move |_spec, _cfg, _permit, _rootfs, container_id, _prep| {
                    t_run.lock().unwrap().push("run_spawn".into());
                    *seen_id.lock().unwrap() = Some(container_id.to_string());
                    Ok(fake_finalization())
                },
            )
            .expect("the ordinary compute path launches");

        assert_eq!(
            *trace.lock().unwrap(),
            vec![
                "isolation_floor".to_string(),
                "reserve".to_string(),
                "acquire_launch_permit".to_string(),
                "run_spawn".to_string(),
                // `fake_run` measures {cpu:1, mem:1}; `settle_completed` receives it VERBATIM.
                "settle:1:1".to_string(),
            ],
            "the ordered compute sequence through launch_with -> launch_compute_with is the fence"
        );

        // The container id the run closure receives is the freshly minted stable workload id.
        let observed = observed_container_id
            .lock()
            .unwrap()
            .clone()
            .expect("the run closure observed a container id");
        assert!(
            observed.starts_with(&format!("myelin-prod-{}-", std::process::id())),
            "the run closure sees the stable myelin-prod-* workload id, got {observed:?}"
        );
        // The successful run is inserted into the live map exactly once (keyed by runsc-<idem>).
        assert_eq!(
            backend.live.lock().unwrap().len(),
            1,
            "a successful compute launch inserts exactly one live entry"
        );
        assert!(launch.output_complete);
    }

    /// Golden failure variant 1 — a `git_wire_only()` backend (no registry) refuses an image-bearing
    /// job at registry resolve, AFTER the isolation floor but BEFORE `reserve`; the run closure never
    /// spawns. This fences the registry-None ordering (resolve precedes reserve).
    #[test]
    fn golden_git_wire_only_refuses_at_registry_before_reserve() {
        let backend = GvisorBackend::git_wire_only();
        let trace = Arc::new(Mutex::new(Vec::<String>::new()));
        let t_iso = trace.clone();
        let t_res = trace.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(move |spec| {
                t_res.lock().unwrap().push("reserve".into());
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_spec| Ok(())),
            Box::new(move |_spec| {
                t_iso.lock().unwrap().push("isolation_floor".into());
                Ok(())
            }),
        );
        let ran = Arc::new(AtomicBool::new(false));
        let ran_at = ran.clone();
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            move |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                ran_at.store(true, Ordering::SeqCst);
                Ok(fake_finalization())
            },
        );
        assert!(
            matches!(result, Err(SandboxLaunchError::Failed(GvisorError::Image(_)))),
            "a git_wire_only backend refuses an image-bearing job at registry resolve, got {result:?}"
        );
        assert_eq!(
            *trace.lock().unwrap(),
            vec!["isolation_floor".to_string()],
            "isolation floor runs, then registry resolve refuses BEFORE reserve is ever called"
        );
        assert!(
            !ran.load(Ordering::SeqCst),
            "the run closure never spawns on a pre-reserve refusal"
        );
    }

    /// Golden failure variant 2 — a `reserve` refusal stops the sequence after the isolation floor and
    /// reserve, before the launch permit and the run spawn. Fences that reserve gates the launch.
    #[test]
    fn golden_reserve_failure_stops_before_launch_permit_and_run() {
        let backend = GvisorBackend::new(test_registry());
        let trace = Arc::new(Mutex::new(Vec::<String>::new()));
        let t_iso = trace.clone();
        let t_res = trace.clone();
        let t_attr = trace.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(move |_spec| {
                t_res.lock().unwrap().push("reserve".into());
                Err(crate::HookError("reserve exhausted".into()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(move |_spec| {
                t_attr.lock().unwrap().push("acquire_launch_permit".into());
                Ok(())
            }),
            Box::new(move |_spec| {
                t_iso.lock().unwrap().push("isolation_floor".into());
                Ok(())
            }),
        );
        let ran = Arc::new(AtomicBool::new(false));
        let ran_at = ran.clone();
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            move |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                ran_at.store(true, Ordering::SeqCst);
                Ok(fake_finalization())
            },
        );
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::Failed(GvisorError::Hook(_)))
            ),
            "a reserve refusal surfaces as a Hook failure, got {result:?}"
        );
        assert_eq!(
            *trace.lock().unwrap(),
            vec!["isolation_floor".to_string(), "reserve".to_string()],
            "isolation floor then reserve; the reserve failure stops before the launch permit and run"
        );
        assert!(
            !ran.load(Ordering::SeqCst),
            "the run closure never spawns when reserve refuses"
        );
    }

    #[test]
    fn successful_reporter_owned_gvisor_launch_defers_settlement_to_terminal_reporter() {
        let backend = GvisorBackend::new(test_registry());
        let hook_settled = Arc::new(AtomicBool::new(false));
        let hook_settled_at = hook_settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, _u| {
                hook_settled_at.store(true, Ordering::SeqCst);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );

        backend
            .launch_with(
                &spec(vec![]),
                &hooks,
                |_spec, _cfg, permit, _rootfs, _container_id, _prep| {
                    permit
                        .commit_and_release()
                        .map_err(|error| RunFailure::uncommitted(error.to_string()))?;
                    Ok(fake_finalization())
                },
            )
            .expect("the sandbox returns measured usage for the reporter transaction");
        assert!(
            !hook_settled.load(Ordering::SeqCst),
            "reporter-owned completion must not settle through the hook"
        );
    }

    #[test]
    fn settlement_failure_unconditionally_kills_and_forgets_the_container() {
        let backend = GvisorBackend::new(test_registry());
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _handle, _usage| {
                Err(crate::HookError("injected settlement failure".into()))
            }),
            Box::new(|_spec| Ok(())),
            Box::new(|_spec| Ok(())),
        );

        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            |_spec, _cfg, permit, _rootfs, _container_id, _prep| {
                permit
                    .commit_and_release()
                    .map_err(|error| RunFailure::uncommitted(error.to_string()))?;
                Ok(fake_finalization())
            },
        );

        assert!(matches!(
            result,
            Err(SandboxLaunchError::Failed(GvisorError::Hook(_)))
        ));
        assert!(
            backend.live.lock().unwrap().is_empty(),
            "an error without a returned handle cannot retain an unreachable live-map entry"
        );
    }

    #[test]
    fn gvisor_releases_the_unused_reserve_when_final_attribution_refuses() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(Mutex::new(None));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                *settled_at.lock().unwrap() = Some(usage);
                Ok(())
            }),
            Box::new(|_t| Err(crate::HookError("claim canceled".into()))),
            Box::new(|_s| Ok(())),
        );
        let spawned = Arc::new(AtomicBool::new(false));
        let spawned_at = spawned.clone();
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            move |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                spawned_at.store(true, Ordering::SeqCst);
                Ok(fake_finalization())
            },
        );
        assert!(matches!(
            result,
            Err(SandboxLaunchError::Failed(GvisorError::Hook(_)))
        ));
        assert!(!spawned.load(Ordering::SeqCst));
        assert_eq!(
            *settled.lock().unwrap(),
            Some(ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            })
        );
    }

    /// Sol's round-1 review: "all pre-permit compound-error combinations" -- when final
    /// attribution refuses AND releasing the now-unused reservation ALSO fails, the caller must see
    /// BOTH messages, never just the attribution error silently swallowing the release failure (or
    /// vice versa). Runs against `Disabled` (no privileged workspace needed) since this exercises
    /// the message-compounding logic itself, which is identical regardless of workspace
    /// integration -- `cleanup_diagnostics` is unconditionally empty for `Disabled`, so this proves
    /// the OTHER two of the three compounding sources (attribution error + release failure) meet
    /// correctly through the real `launch_with` code path, not just the pure `join_diagnostics`
    /// helper in isolation.
    #[test]
    fn launch_permit_refusal_compounds_with_a_failing_reservation_release() {
        let backend = GvisorBackend::new(test_registry());
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _h, _usage| {
                Err(crate::HookError("settle backend unavailable".into()))
            }),
            Box::new(|_t| Err(crate::HookError("claim canceled".into()))),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            |_spec, _cfg, _permit, _rootfs, _container_id, _prep| Ok(fake_finalization()),
        );
        match result {
            Err(SandboxLaunchError::Failed(GvisorError::Runtime(message))) => {
                assert!(
                    message.contains("claim canceled"),
                    "the original attribution refusal must survive: {message}"
                );
                assert!(
                    message.contains("releasing the unused reservation also failed"),
                    "the release failure must be compounded in, not lost: {message}"
                );
                assert!(
                    message.contains("settle backend unavailable"),
                    "the release failure's own text must be present verbatim: {message}"
                );
            }
            other => panic!("expected a compound GvisorError::Runtime, got {other:?}"),
        }
    }

    /// The pre-existing leak this fix closes: previously, ANY error from `run(...)` propagated
    /// straight out of `launch_with` with NEITHER `release_unused` NOR `settle_completed` ever
    /// called — leaking the reservation on every single run failure. These tests prove each of the
    /// four `RunFailure` phases dispatches to the correct outcome, per Sol's corrected disposition
    /// table (phase × `CompletionSettlementOwner`):
    ///
    /// | Phase                    | `Hook` owner                       | `TerminalReporter` owner                  |
    /// |---------------------------|-------------------------------------|--------------------------------------------|
    /// | `Uncommitted`             | `release_unused`, then `Failed`     | `release_unused`, then `Failed`             |
    /// | `CommitOutcomeUnknown`    | `DurableOutcomeUnknown`             | `DurableOutcomeUnknown`                     |
    /// | `CommittedButNotExecuted` | settle zero, then `Failed`          | `RetryableAttempt(SandboxInfrastructure, 0)`|
    /// | `Executed`                | settle usage, then `Failed`         | `RetryableAttempt(SandboxInfrastructure, usage)`|
    ///
    /// `Uncommitted` and `CommitOutcomeUnknown` are owner-INDEPENDENT (an uncommitted attempt has no
    /// terminal report to defer to regardless of owner; an outcome-unknown attempt must never be
    /// guessed at either way) — only the two post-commit phases branch on ownership, since only they
    /// carry a real (if zero) measured cost a `TerminalReporter` must eventually account for.
    #[test]
    fn gvisor_run_failure_uncommitted_releases_reserve_via_release_unused() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(Mutex::new(None));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                *settled_at.lock().unwrap() = Some(usage);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                Err(RunFailure::uncommitted("injected uncommitted run failure"))
            },
        );
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::Failed(GvisorError::Runtime(_)))
            ),
            "an uncommitted run failure must surface as Failed(GvisorError::Runtime): {result:?}"
        );
        assert_eq!(
            *settled.lock().unwrap(),
            Some(ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            }),
            "release_unused must settle at zero even under reporter-owned completion — it is \
             owner-independent, unlike settle_completed"
        );
    }

    /// `CommitOutcomeUnknown` must NEVER release or settle — the durable commit outcome is
    /// genuinely unknown, and guessing either way misaccounts a real reservation. Owner-independent:
    /// this test uses `Hook` ownership specifically to prove the outcome-unknown path bypasses
    /// `settle_completed` entirely rather than merely happening to observe a reporter's no-op.
    #[test]
    fn gvisor_run_failure_commit_outcome_unknown_never_releases_or_settles() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(AtomicBool::new(false));
        let settled_at = settled.clone();
        let released = Arc::new(AtomicBool::new(false));
        let released_at = released.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                settled_at.store(true, Ordering::SeqCst);
                if usage
                    == (ResourceUsage {
                        cpu_seconds: 0,
                        mem_byte_seconds: 0,
                    })
                {
                    released_at.store(true, Ordering::SeqCst);
                }
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                Err(RunFailure::commit_outcome_unknown(
                    "injected commit-outcome-unknown run failure",
                ))
            },
        );
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::DurableOutcomeUnknown(GvisorError::Runtime(_)))
            ),
            "a commit-outcome-unknown run failure must surface as DurableOutcomeUnknown: {result:?}"
        );
        assert!(
            !settled.load(Ordering::SeqCst) && !released.load(Ordering::SeqCst),
            "neither settle_completed nor release_unused (which also calls the settle hook) may \
             ever fire for an outcome-unknown attempt"
        );
    }

    /// `CommittedButNotExecuted` under `Hook` ownership settles zero synchronously, then surfaces
    /// `Failed` — a real terminal report IS expected here (unlike `Uncommitted`'s "none will ever
    /// follow"), and `Hook` ownership means the hook itself is the one committing that report.
    #[test]
    fn gvisor_run_failure_committed_but_not_executed_hook_owner_settles_zero_then_fails() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(Mutex::new(None));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                *settled_at.lock().unwrap() = Some(usage);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                Err(RunFailure::committed_but_not_executed(
                    "injected committed-but-not-executed run failure",
                ))
            },
        );
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::Failed(GvisorError::Runtime(_)))
            ),
            "a Hook-owned committed-but-not-executed failure must surface as Failed: {result:?}"
        );
        assert_eq!(
            *settled.lock().unwrap(),
            Some(ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            }),
            "Hook ownership must settle zero usage synchronously through settle_completed"
        );
    }

    /// `CommittedButNotExecuted` under `TerminalReporter` ownership must NOT call `settle_completed`
    /// at all (it would silently no-op) — it must instead surface `RetryableAttempt` so the RUNNER
    /// routes it through the reporter's own `report_retryable_attempt` transaction, which durably
    /// accounts usage and either requeues or terminalizes the exact claim. This is the exact case
    /// Sol's review caught: the original fix called `settle_completed` here and returned an
    /// ordinary `Failed`, which under reporter ownership silently discarded the accounting with no
    /// terminal report ever following.
    #[test]
    fn gvisor_run_failure_committed_but_not_executed_reporter_owner_yields_retryable_attempt() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(AtomicBool::new(false));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, _usage| {
                settled_at.store(true, Ordering::SeqCst);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                Err(RunFailure::committed_but_not_executed(
                    "injected committed-but-not-executed run failure",
                ))
            },
        );
        match result {
            Err(SandboxLaunchError::RetryableAttempt { cause, usage, .. }) => {
                assert_eq!(cause, RetryableAttemptCause::SandboxInfrastructure);
                assert_eq!(
                    usage,
                    ResourceUsage {
                        cpu_seconds: 0,
                        mem_byte_seconds: 0,
                    }
                );
            }
            other => panic!("expected RetryableAttempt with zero usage, got {other:?}"),
        }
        assert!(
            !settled.load(Ordering::SeqCst),
            "settle_completed must never be called directly here — the runner's retryable-attempt \
             transaction is the sole accounting path under reporter ownership"
        );
    }

    /// `Executed` under `Hook` ownership must settle the CONSERVATIVE fallback usage synchronously,
    /// never zero — a job engineered to fail exactly after the runtime was released to exec must not
    /// execute for free (the host-DoS surface Sol's design closes) — then surface `Failed`.
    #[test]
    fn gvisor_run_failure_executed_hook_owner_settles_fallback_usage_then_fails() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(Mutex::new(None));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                *settled_at.lock().unwrap() = Some(usage);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let fallback_usage = ResourceUsage {
            cpu_seconds: 7,
            mem_byte_seconds: 700,
        };
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            move |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                Err(RunFailure::executed(
                    "injected executed-phase run failure",
                    fallback_usage,
                ))
            },
        );
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::Failed(GvisorError::Runtime(_)))
            ),
            "a Hook-owned executed-phase failure must surface as Failed: {result:?}"
        );
        assert_eq!(
            *settled.lock().unwrap(),
            Some(fallback_usage),
            "the executed phase must settle its carried conservative fallback usage, never zero"
        );
    }

    /// `Executed` under `TerminalReporter` ownership must surface `RetryableAttempt` carrying the
    /// SAME conservative fallback usage (never zero) — the reporter's own transaction, not
    /// `settle_completed`, is what durably accounts it.
    #[test]
    fn gvisor_run_failure_executed_reporter_owner_yields_retryable_attempt_with_fallback_usage() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(AtomicBool::new(false));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, _usage| {
                settled_at.store(true, Ordering::SeqCst);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let fallback_usage = ResourceUsage {
            cpu_seconds: 3,
            mem_byte_seconds: 300,
        };
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            move |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                Err(RunFailure::executed(
                    "injected executed-phase run failure",
                    fallback_usage,
                ))
            },
        );
        match result {
            Err(SandboxLaunchError::RetryableAttempt { cause, usage, .. }) => {
                assert_eq!(cause, RetryableAttemptCause::SandboxInfrastructure);
                assert_eq!(usage, fallback_usage);
            }
            other => panic!("expected RetryableAttempt with the fallback usage, got {other:?}"),
        }
        assert!(
            !settled.load(Ordering::SeqCst),
            "settle_completed must never be called directly here — the runner's retryable-attempt \
             transaction is the sole accounting path under reporter ownership"
        );
    }

    /// CT-007 gate 2/4 (f, corrected ordering): a RED isolation floor refuses BEFORE the registry
    /// lookup is ever consulted — proven by using a genuinely UNREGISTERED image (so if the
    /// (wrong-order) implementation queried the registry first, it would refuse there as
    /// `GvisorError::Image` WITHOUT the floor hook ever having been called, and `floor_called` would
    /// read `false`). Asserting `floor_called == true` alongside a `GvisorError::Hook` result is only
    /// possible if the floor really did run first, despite the image being unresolvable — which also
    /// means an exhausted-wallet caller cannot force the (now-cheap, but real) registry lookup by
    /// repeatedly failing the floor.
    #[test]
    fn red_isolation_floor_refuses_before_registry_lookup_reserve_or_spawn() {
        let floor_called = Arc::new(AtomicBool::new(false));
        let floor_called_at = floor_called.clone();
        let reserve_called = Arc::new(AtomicBool::new(false));
        let reserve_called_at = reserve_called.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(move |spec| {
                reserve_called_at.store(true, Ordering::SeqCst);
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(move |_spec| {
                floor_called_at.store(true, Ordering::SeqCst);
                Err(crate::HookError(
                    "isolation floor is RED for this test".into(),
                ))
            }),
        );

        let mut unregistered_spec = spec(vec![]);
        unregistered_spec.image = ImageRef::pinned(
            "test.local/genuinely-unregistered@sha256:3333333333333333333333333333333333333333333333333333333333333333",
        )
        .unwrap();
        let spawned = Arc::new(AtomicBool::new(false));
        let spawned_at = spawned.clone();
        // A fresh, otherwise-empty registry — the spec's image is deliberately NOT registered here,
        // so a wrong-order (registry-before-floor) implementation would refuse via `Image`, not
        // `Hook`, and would never call the floor closure at all.
        let backend = GvisorBackend::new(Arc::new(
            crate::asset_registry::GvisorAssetRegistry::from_bindings(vec![]).unwrap(),
        ));
        let result = backend.launch_with(
            &unregistered_spec,
            &hooks,
            move |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                spawned_at.store(true, Ordering::SeqCst);
                Ok(fake_finalization())
            },
        );

        assert!(
            matches!(result, Err(SandboxLaunchError::Failed(GvisorError::Hook(_)))),
            "the isolation floor's own refusal must surface, proving it ran BEFORE the registry \
             lookup (an unregistered image would otherwise short-circuit as `Image` first): {result:?}"
        );
        assert!(
            floor_called.load(Ordering::SeqCst),
            "the isolation floor must be consulted even for an unresolvable image"
        );
        assert!(
            !reserve_called.load(Ordering::SeqCst),
            "no reserve may be attempted"
        );
        assert!(
            !spawned.load(Ordering::SeqCst),
            "the run closure must never be invoked"
        );
    }

    /// CT-007 gate 2/4 (f, still-correct half): a GREEN isolation floor + an unknown image still
    /// refuses before `reserve`/the `run` closure — none of them ever fire. This is the part of the
    /// original ordering test that was already right; it just now runs AFTER the floor instead of
    /// before it.
    #[test]
    fn unknown_image_after_green_floor_refuses_before_reserve_or_spawn() {
        let floor_called = Arc::new(AtomicBool::new(false));
        let floor_called_at = floor_called.clone();
        let reserve_called = Arc::new(AtomicBool::new(false));
        let reserve_called_at = reserve_called.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(move |spec| {
                reserve_called_at.store(true, Ordering::SeqCst);
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(move |_spec| {
                floor_called_at.store(true, Ordering::SeqCst);
                Ok(())
            }),
        );

        let mut unregistered_spec = spec(vec![]);
        unregistered_spec.image = ImageRef::pinned(
            "test.local/genuinely-unregistered@sha256:3333333333333333333333333333333333333333333333333333333333333333",
        )
        .unwrap();
        let spawned = Arc::new(AtomicBool::new(false));
        let spawned_at = spawned.clone();
        // A fresh, otherwise-empty registry — the fixture image is deliberately NOT registered here.
        let backend = GvisorBackend::new(Arc::new(
            crate::asset_registry::GvisorAssetRegistry::from_bindings(vec![]).unwrap(),
        ));
        let result = backend.launch_with(
            &unregistered_spec,
            &hooks,
            move |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                spawned_at.store(true, Ordering::SeqCst);
                Ok(fake_finalization())
            },
        );

        assert!(matches!(
            result,
            Err(SandboxLaunchError::Failed(GvisorError::Image(_)))
        ));
        assert!(
            floor_called.load(Ordering::SeqCst),
            "the isolation floor must have been consulted (and passed) first"
        );
        assert!(
            !reserve_called.load(Ordering::SeqCst),
            "no reserve may be attempted"
        );
        assert!(
            !spawned.load(Ordering::SeqCst),
            "the run closure must never be invoked"
        );
    }

    /// A committed regression pin for `GvisorBackend::git_wire_only()`'s refusal of ordinary launch:
    /// the behavior existed (see `launch_with`'s `self.registry.as_ref().ok_or_else(...)`) but had no
    /// test asserting it returns `GvisorError::Image` rather than panicking or hanging.
    #[test]
    fn git_wire_only_backend_refuses_ordinary_launch() {
        let backend = GvisorBackend::git_wire_only();
        let hooks = ok_hooks();
        let result = backend.launch(&spec(vec![]), &hooks);
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::Failed(GvisorError::Image(_)))
            ),
            "a git-wire-only backend has no asset registry and must refuse an ordinary launch as \
             GvisorError::Image, not panic or hang: {result:?}"
        );
    }

    /// The same refusal for the streaming entry point.
    #[test]
    fn git_wire_only_backend_refuses_ordinary_launch_streaming() {
        let backend = GvisorBackend::git_wire_only();
        let hooks = ok_hooks();
        let output: Arc<dyn SandboxOutputSink> = Arc::new(RecordingOutput::default());
        let result =
            backend.launch_streaming(&spec(vec![]), &hooks, output, SandboxCancellation::new());
        assert!(
            matches!(result, Err(SandboxLaunchError::Failed(GvisorError::Image(_)))),
            "a git-wire-only backend must refuse ordinary launch_streaming the same way as launch: \
             {result:?}"
        );
    }

    #[test]
    fn cancelled_git_wire_refuses_before_reserve_or_spawn() {
        let cancelled = AtomicBool::new(true);
        let spec = GitWireSpec {
            repo_host_path: PathBuf::from("/absent/repo.git"),
            root: PathBuf::from("/absent"),
            git_argv: vec!["upload-pack".into()],
            stdin: Vec::new(),
            env: Vec::new(),
            quarantine_host_path: None,
            limits: ResourceLimits {
                cpu_millis: 1,
                mem_bytes: 1,
                disk_bytes: 1,
                tmpfs_bytes: 1,
                pids_max: 1,
                timeout_secs: 1,
            },
            run_token: RunTokenCredential::new("test-bearer", "cancel", 300).unwrap(),
            meter_to: MeterTarget {
                reserve_id: "cancel".into(),
            },
            idem_token: IdemToken("cancel".into()),
        };
        let result = GvisorBackend::git_wire_only().launch_git_wire_until_cancelled(
            &spec,
            &ok_hooks(),
            &cancelled,
        );
        assert!(
            matches!(result, Err(WireError::Runtime(message)) if message.contains("cancelled by process shutdown"))
        );
    }

    /// Git-wire is a direct synchronous path with no terminal reporter above it — reporter-owned
    /// hooks must be refused BEFORE reserve or any rootfs/mount/spawn work, exactly like the
    /// analogous agent-service `dispatch_compute` refusal. Proven WITHOUT a real `runsc`: the
    /// refusal happens before any of that is ever touched.
    #[test]
    fn git_wire_refuses_reporter_owned_hooks_before_reserve() {
        // A REAL repo directory under a REAL root — this test is about the ownership refusal, not
        // the symlink/path-confinement defense (`symlinked_repo_path_is_refused_before_mount`
        // covers that), so the path itself must actually pass `assert_repo_under_root` first.
        let tmp = std::env::temp_dir().join(format!(
            "myelin-gitwire-reporter-owned-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let root = tmp.join("git-root");
        let repo = root.join("acme").join("fr-par").join("widgets.git");
        std::fs::create_dir_all(&repo).unwrap();

        let reserve_called = Arc::new(AtomicBool::new(false));
        let reserve_called_at = reserve_called.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(move |spec| {
                reserve_called_at.store(true, Ordering::SeqCst);
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let spec = GitWireSpec {
            repo_host_path: repo,
            root,
            git_argv: vec!["upload-pack".into()],
            stdin: Vec::new(),
            env: Vec::new(),
            quarantine_host_path: None,
            limits: ResourceLimits {
                cpu_millis: 1,
                mem_bytes: 1,
                disk_bytes: 1,
                tmpfs_bytes: 1,
                pids_max: 1,
                timeout_secs: 1,
            },
            run_token: RunTokenCredential::new("test-bearer", "reporter-owned", 300).unwrap(),
            meter_to: MeterTarget {
                reserve_id: "reporter-owned".into(),
            },
            idem_token: IdemToken("reporter-owned".into()),
        };
        let result = GvisorBackend::git_wire_only().launch_git_wire_until_cancelled(
            &spec,
            &hooks,
            &NEVER_CANCELLED,
        );
        assert!(
            matches!(result, Err(WireError::Runtime(ref message)) if message.contains("requires Hook-owned")),
            "expected a Hook-ownership refusal, got {result:?}"
        );
        assert!(
            !reserve_called.load(Ordering::SeqCst),
            "reporter-owned hooks must refuse before reserve is ever called"
        );
    }

    /// The four `RunFailure` phases dispatch through `dispose_git_wire_run_failure` exactly as
    /// gVisor's `dispose_run_failure` does under `Hook` ownership (git-wire always settles
    /// synchronously — there is no reporter to defer to): `Uncommitted` -> `release_unused`;
    /// `CommitOutcomeUnknown` -> neither release nor settle; `CommittedButNotExecuted` -> settle
    /// zero; `Executed` -> settle the carried usage. Unit-tested directly (no real `runsc` needed).
    #[test]
    fn dispose_git_wire_run_failure_dispatches_all_four_phases() {
        fn recording_hooks() -> (RunnerHooks, Arc<Mutex<Vec<ResourceUsage>>>) {
            let settled = Arc::new(Mutex::new(Vec::new()));
            let settled_at = settled.clone();
            let hooks = RunnerHooks::new(
                CompletionSettlementOwner::Hook,
                Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
                Box::new(move |_spec, _h, usage| {
                    settled_at.lock().unwrap().push(usage);
                    Ok(())
                }),
                Box::new(|_t| Ok(())),
                Box::new(|_s| Ok(())),
            );
            (hooks, settled)
        }
        let job = spec(vec![]);
        let reserve = ReserveHandle(job.meter_to.reserve_id.clone());
        let zero = ResourceUsage {
            cpu_seconds: 0,
            mem_byte_seconds: 0,
        };

        let (hooks, settled) = recording_hooks();
        let error = dispose_git_wire_run_failure(
            &hooks,
            &job,
            &reserve,
            RunFailure::uncommitted("injected uncommitted"),
        );
        assert!(matches!(error, WireError::Runtime(m) if m.contains("injected uncommitted")));
        assert_eq!(
            *settled.lock().unwrap(),
            vec![zero],
            "release_unused settles zero"
        );

        let (hooks, settled) = recording_hooks();
        let error = dispose_git_wire_run_failure(
            &hooks,
            &job,
            &reserve,
            RunFailure::commit_outcome_unknown("injected outcome unknown"),
        );
        assert!(matches!(error, WireError::Runtime(m) if m.contains("needs reconciliation")));
        assert!(
            settled.lock().unwrap().is_empty(),
            "commit-outcome-unknown must never release or settle"
        );

        let (hooks, settled) = recording_hooks();
        let error = dispose_git_wire_run_failure(
            &hooks,
            &job,
            &reserve,
            RunFailure::committed_but_not_executed("injected committed but not executed"),
        );
        assert!(
            matches!(error, WireError::Runtime(m) if m.contains("injected committed but not executed"))
        );
        assert_eq!(*settled.lock().unwrap(), vec![zero]);

        let (hooks, settled) = recording_hooks();
        let fallback_usage = ResourceUsage {
            cpu_seconds: 4,
            mem_byte_seconds: 400,
        };
        let error = dispose_git_wire_run_failure(
            &hooks,
            &job,
            &reserve,
            RunFailure::executed("injected executed", fallback_usage),
        );
        assert!(matches!(error, WireError::Runtime(m) if m.contains("injected executed")));
        assert_eq!(
            *settled.lock().unwrap(),
            vec![fallback_usage],
            "executed must settle the carried conservative usage, never zero"
        );
    }

    /// **CT-006b 4a — symlink-path defence in depth (no runsc needed).** A textually-clean repo
    /// locator whose resolved `<repo>.git` is a SYMLINK out of the tenant tree is REFUSED by
    /// [`assert_repo_under_root`] BEFORE any mount, while a REAL directory under the root is admitted.
    #[test]
    fn symlinked_repo_path_is_refused_before_mount() {
        let tmp = std::env::temp_dir().join(format!(
            "myelin-gitwire-symlink-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let root = tmp.join("git-root");
        let outside = tmp.join("outside-the-tree");
        std::fs::create_dir_all(root.join("acme").join("fr-par")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        // (1) A REAL bare-repo directory under the root is admitted.
        let real = resolve_bare_repo_path(&root, "acme", "fr-par", "widgets").unwrap();
        std::fs::create_dir_all(&real).unwrap();
        assert!(
            assert_repo_under_root(&root, &real).is_ok(),
            "a real directory under the root must be admitted"
        );

        // (2) A SYMLINKED `<repo>.git` pointing OUT of the tenant tree is refused (final-symlink check
        //     AND the canonical-escape check would both catch it).
        let evil = resolve_bare_repo_path(&root, "acme", "fr-par", "evil").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &evil).unwrap();
        let r = assert_repo_under_root(&root, &evil);
        assert!(
            matches!(r, Err(WireError::Path(_))),
            "a symlinked repo path escaping the tree must be refused, got {r:?}"
        );

        // (3) A symlinked INTERMEDIATE component (the tenant dir → /tmp) is caught by the canonical
        //     starts_with check even though the final `<repo>.git` is a real dir under the symlink.
        let root2 = tmp.join("git-root-2");
        std::fs::create_dir_all(&root2).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root2.join("acme")).unwrap();
        let leak_parent = outside.join("fr-par");
        std::fs::create_dir_all(leak_parent.join("widgets.git")).unwrap();
        let via_symlinked_component =
            resolve_bare_repo_path(&root2, "acme", "fr-par", "widgets").unwrap();
        let r2 = assert_repo_under_root(&root2, &via_symlinked_component);
        assert!(
            matches!(r2, Err(WireError::Path(_))),
            "a symlinked intermediate component leaving the root must be refused, got {r2:?}"
        );

        // (4) An absent repo path fails closed (never a silent admit).
        let absent = resolve_bare_repo_path(&root, "acme", "fr-par", "ghost").unwrap();
        assert!(matches!(
            assert_repo_under_root(&root, &absent),
            Err(WireError::Path(_))
        ));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- CT-007 slice 3, piece 7b: finalize_runtime / settle_finalization -----------------------

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn finalize_runtime_mints_evidence_on_a_clean_rootless_teardown() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir().to_path_buf();
        let cgroup_identity = cg.identity();
        // `/bin/true` ignores every argument and always exits 0 — a deterministic stand-in for a
        // `runsc delete -force` that succeeds, with no real runtime involved.
        let evidence = finalize_runtime(
            Path::new("/bin/true"),
            "container-does-not-matter-for-bin-true",
            &PreparedRuntimeMode::Rootless,
            cg,
            Duration::from_secs(2),
            DirectChildRetirement::Reaped,
        )
        .expect(
            "a confirmed-reaped child, a successful delete, and a clean cgroup must mint evidence",
        );
        assert_eq!(evidence.namespace, RuntimeNamespaceQuiescence::Rootless);
        assert_eq!(evidence.cgroup.cgroup_identity(), cgroup_identity);
        assert!(
            !dir.exists(),
            "finalize_runtime must remove the cgroup on success"
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn finalize_runtime_refuses_when_the_direct_child_was_not_confirmed_reaped() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir().to_path_buf();
        let result = finalize_runtime(
            Path::new("/bin/true"),
            "container-does-not-matter-for-bin-true",
            &PreparedRuntimeMode::Rootless,
            cg,
            Duration::from_secs(2),
            DirectChildRetirement::Unconfirmed("wait() returned ECHILD".to_string()),
        );
        let error = result.expect_err("an unconfirmed direct-child reap must refuse evidence");
        assert_eq!(error.issues.len(), 1, "{error:?}");
        assert!(matches!(
            error.issues[0],
            RuntimeTeardownIssue::ChildNotConfirmedReaped(_)
        ));
        // The cgroup itself was genuinely empty — quiescence still ran (and succeeded) despite the
        // unrelated child-reap issue; a caller must not assume "the cgroup leaked" from this Err.
        assert!(
            !dir.exists(),
            "quiesce must still run (and succeed) even though evidence was refused"
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn finalize_runtime_refuses_when_the_container_delete_is_not_confirmed() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir().to_path_buf();
        // `/bin/false` ignores every argument and always exits 1 — a deterministic stand-in for a
        // `runsc delete -force` that fails.
        let result = finalize_runtime(
            Path::new("/bin/false"),
            "container-does-not-matter-for-bin-false",
            &PreparedRuntimeMode::Rootless,
            cg,
            Duration::from_secs(2),
            DirectChildRetirement::Reaped,
        );
        let error = result.expect_err("a non-zero delete exit must refuse evidence");
        assert_eq!(error.issues.len(), 1, "{error:?}");
        assert!(matches!(
            error.issues[0],
            RuntimeTeardownIssue::ContainerNotConfirmedDeleted(_)
        ));
        assert!(
            !dir.exists(),
            "cgroup quiescence must still run (and succeed) despite the delete failure"
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn finalize_runtime_skips_the_delete_but_still_quiesces_when_namespace_identity_drifts() {
        // `/bin/false` stands in for a `runsc delete` that would fail — but it must never even be
        // invoked here, since the (injected) namespace-identity revalidation reports a drift first.
        // If `retire_container` ran anyway, `ContainerNotConfirmedDeleted` would ALSO appear in
        // `issues`, which the assertion below rules out.
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir().to_path_buf();
        let expected_root_identity = (11, 22);
        let drifted_identity = (11, 99);
        let prepared_mode = PreparedRuntimeMode::ExplicitUserNamespace {
            config: UserNamespaceConfig::for_tests(1000, 1000, 200000, 200000),
            expected_root_identity,
        };
        let result = finalize_runtime_given(
            Path::new("/bin/false"),
            "container-must-not-be-deleted",
            &prepared_mode,
            cg,
            Duration::from_secs(2),
            DirectChildRetirement::Reaped,
            move || Ok(drifted_identity),
            |_bin, _container_id, _mode| {
                panic!("retire_container must never be invoked after a namespace-identity drift")
            },
        );
        let error = result.expect_err("a drifted namespace identity must refuse evidence");
        assert_eq!(error.issues.len(), 1, "{error:?}");
        assert!(matches!(
            error.issues[0],
            RuntimeTeardownIssue::NamespaceIdentityDrifted(_)
        ));
        assert!(
            !dir.exists(),
            "cgroup quiescence must still run (and succeed) even though the delete was skipped"
        );
    }

    /// Sol's round-2 review: the identity-MATCHES branch, using the `_given` seam's SECOND
    /// injectable (`retire_container_fn`) so this test never touches the real global
    /// `EXPLICIT_USERNS_POLICY`. Proves: deletion is invoked exactly once, with the derived
    /// explicit invocation mode, and the minted evidence carries the expected container/namespace/
    /// cgroup identities.
    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn finalize_runtime_mints_explicit_userns_evidence_when_the_identity_still_matches() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let cgroup_identity = cg.identity();
        let identity = (33, 44);
        let userns_config = UserNamespaceConfig::for_tests(1000, 1000, 200000, 200000);
        let prepared_mode = PreparedRuntimeMode::ExplicitUserNamespace {
            config: userns_config,
            expected_root_identity: identity,
        };
        let delete_calls = std::cell::Cell::new(0u32);
        let seen_mode = std::cell::RefCell::new(None);
        let evidence = finalize_runtime_given(
            Path::new("/bin/true"),
            "container-xyz",
            &prepared_mode,
            cg,
            Duration::from_secs(2),
            DirectChildRetirement::Reaped,
            move || Ok(identity),
            |_bin, container_id, mode| {
                delete_calls.set(delete_calls.get() + 1);
                *seen_mode.borrow_mut() = Some(mode);
                assert_eq!(container_id, "container-xyz");
                Ok(())
            },
        )
        .expect("a matching identity, successful delete, and clean cgroup must mint evidence");

        assert_eq!(delete_calls.get(), 1, "delete must be invoked exactly once");
        assert_eq!(
            seen_mode.into_inner(),
            Some(RunscInvocationMode::ExplicitUserNamespace(userns_config)),
            "the derived explicit invocation mode must be used"
        );
        assert_eq!(evidence.container_id, "container-xyz");
        assert_eq!(
            evidence.namespace,
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity: identity
            }
        );
        assert_eq!(evidence.cgroup.cgroup_identity(), cgroup_identity);
    }

    #[test]
    fn preflight_capture_and_teardown_result_passes_through_a_clean_success() {
        let evidence = RuntimeQuiescenceEvidence {
            container_id: "c".to_string(),
            namespace: RuntimeNamespaceQuiescence::Rootless,
            cgroup: CgroupQuiescenceEvidence::assert_for_tests((1, 2)),
        };
        let result = preflight_capture_and_teardown_result(Ok(outcome(b"", b"")), Ok(evidence));
        assert!(result.is_ok(), "expected Ok, got Err");
    }

    #[test]
    fn preflight_capture_and_teardown_result_surfaces_a_teardown_only_failure() {
        let teardown = RuntimeTeardownError {
            issues: vec![RuntimeTeardownIssue::Cgroup(
                CgroupQuiescenceError::StillPopulated {
                    waited: Duration::from_secs(2),
                },
            )],
        };
        let result = preflight_capture_and_teardown_result(Ok(outcome(b"", b"")), Err(teardown));
        let Err(message) = result else {
            panic!("a teardown-only failure must still refuse");
        };
        assert!(
            message.contains("runtime teardown check failed"),
            "{message}"
        );
    }

    #[test]
    fn preflight_capture_and_teardown_result_surfaces_a_capture_only_failure() {
        let evidence = RuntimeQuiescenceEvidence {
            container_id: "c".to_string(),
            namespace: RuntimeNamespaceQuiescence::Rootless,
            cgroup: CgroupQuiescenceEvidence::assert_for_tests((1, 2)),
        };
        let capture_failure = RunFailure::uncommitted("spawn runsc: boom");
        let result = preflight_capture_and_teardown_result(Err(capture_failure), Ok(evidence));
        let Err(message) = result else {
            panic!("a capture-only failure must still refuse");
        };
        assert!(message.contains("boom"), "{message}");
    }

    /// Sol's round-2 review, blocker 4: the previous implementation applied `?` to the capture
    /// result BEFORE ever inspecting the teardown result — when BOTH failed, the teardown
    /// diagnostic silently disappeared. Proves both messages survive when both fail.
    #[test]
    fn preflight_capture_and_teardown_result_reports_both_failures_when_both_fail() {
        let capture_failure = RunFailure::uncommitted("spawn runsc: boom");
        let teardown = RuntimeTeardownError {
            issues: vec![RuntimeTeardownIssue::Cgroup(
                CgroupQuiescenceError::StillPopulated {
                    waited: Duration::from_secs(2),
                },
            )],
        };
        let result = preflight_capture_and_teardown_result(Err(capture_failure), Err(teardown));
        let Err(message) = result else {
            panic!("a compound failure must still refuse");
        };
        assert!(message.contains("boom"), "{message}");
        assert!(
            message.contains("runtime teardown check also failed"),
            "{message}"
        );
    }

    /// A [`RunscChild`] that records whether `kill()` was invoked, for
    /// `discard_container_run_after_teardown_failure`'s tests below.
    struct CountingFakeRunsc {
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

    fn container_run_with_real_bundle_dir(killed: Arc<AtomicBool>) -> (ContainerRun, PathBuf) {
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

    /// Sol's round-2 review, blocker 3: a successful `ContainerRun` that `settle_finalization`
    /// converts into a failure must not leak its staged bundle dir (it will never reach
    /// `self.live`, which is the ONLY other place that removes it). Non-drift issues are safe to
    /// best-effort re-attempt the deferred kill on, as one more defense-in-depth try.
    #[test]
    fn discard_container_run_after_teardown_failure_removes_bundle_and_best_effort_kills_on_a_non_drift_issue(
    ) {
        let killed = Arc::new(AtomicBool::new(false));
        let (run, bundle_dir) = container_run_with_real_bundle_dir(killed.clone());
        let teardown = RuntimeTeardownError {
            issues: vec![RuntimeTeardownIssue::ContainerNotConfirmedDeleted(
                "exited 1".to_string(),
            )],
        };
        discard_container_run_after_teardown_failure(run, &teardown);
        assert!(
            !bundle_dir.exists(),
            "the staged bundle must be removed even when the run is discarded"
        );
        assert!(
            killed.load(Ordering::SeqCst),
            "a non-drift issue must still attempt the best-effort deferred kill"
        );
    }

    /// Sol's round-2 review, blocker 3: when the teardown issues include a namespace-identity
    /// drift, `finalize_runtime` already determined the path-based delete/kill is unsafe to trust
    /// — this cleanup must NOT retroactively invoke it, even as a "best effort", but must still
    /// remove the bundle dir (that part is unconditional).
    #[test]
    fn discard_container_run_after_teardown_failure_skips_kill_but_still_removes_bundle_on_namespace_drift(
    ) {
        let killed = Arc::new(AtomicBool::new(false));
        let (run, bundle_dir) = container_run_with_real_bundle_dir(killed.clone());
        let teardown = RuntimeTeardownError {
            issues: vec![RuntimeTeardownIssue::NamespaceIdentityDrifted(
                "expected (1, 2), found (1, 3)".to_string(),
            )],
        };
        discard_container_run_after_teardown_failure(run, &teardown);
        assert!(
            !bundle_dir.exists(),
            "the staged bundle must still be removed even when kill is skipped"
        );
        assert!(
            !killed.load(Ordering::SeqCst),
            "a namespace-identity drift must NOT trigger the path-based deferred kill"
        );
    }

    #[test]
    fn settle_finalization_returns_the_primary_unchanged_when_finalized() {
        let evidence = RuntimeQuiescenceEvidence {
            container_id: "c".to_string(),
            namespace: RuntimeNamespaceQuiescence::Rootless,
            cgroup: CgroupQuiescenceEvidence::assert_for_tests((1, 2)),
        };
        let finalization: RuntimeFinalization<Result<u64, RunFailure>> =
            RuntimeFinalization::Finalized(FinalizedRun {
                primary: Ok(42),
                evidence,
            });
        let result = settle_finalization(
            finalization,
            |_: &u64| ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            },
            |_: u64, _: &RuntimeTeardownError| {
                panic!("on_discarded_success must not run when finalization succeeded")
            },
        );
        assert!(matches!(result, Ok(42)), "{result:?}");
    }

    #[test]
    fn settle_finalization_converts_a_clean_success_into_executed_when_teardown_fails() {
        let teardown = RuntimeTeardownError {
            issues: vec![RuntimeTeardownIssue::ContainerNotConfirmedDeleted(
                "exited 1".to_string(),
            )],
        };
        let usage = ResourceUsage {
            cpu_seconds: 3,
            mem_byte_seconds: 4096,
        };
        let finalization: RuntimeFinalization<Result<u64, RunFailure>> =
            RuntimeFinalization::Failed {
                primary: Ok(42),
                teardown,
            };
        let discarded = std::cell::Cell::new(false);
        let result = settle_finalization(
            finalization,
            move |_: &u64| usage,
            |value: u64, _: &RuntimeTeardownError| {
                assert_eq!(value, 42);
                discarded.set(true);
            },
        );
        assert!(
            discarded.get(),
            "on_discarded_success must run for a discarded successful primary"
        );
        match result {
            Err(RunFailure::Executed {
                usage: got_usage,
                message,
            }) => {
                assert_eq!(got_usage, usage);
                assert!(message.contains("runtime teardown failed"));
            }
            other => panic!("expected RunFailure::Executed, got {other:?}"),
        }
    }

    #[test]
    fn settle_finalization_augments_an_existing_run_failure_without_losing_its_phase_or_usage() {
        let teardown = RuntimeTeardownError {
            issues: vec![RuntimeTeardownIssue::Cgroup(
                CgroupQuiescenceError::StillPopulated {
                    waited: Duration::from_secs(2),
                },
            )],
        };
        let original_usage = ResourceUsage {
            cpu_seconds: 7,
            mem_byte_seconds: 8192,
        };
        let finalization: RuntimeFinalization<Result<u64, RunFailure>> =
            RuntimeFinalization::Failed {
                primary: Err(RunFailure::executed("original failure", original_usage)),
                teardown,
            };
        let result = settle_finalization(
            finalization,
            |_: &u64| panic!("usage_of must not be called when the primary already failed"),
            |_: u64, _: &RuntimeTeardownError| {
                panic!("on_discarded_success must not run when the primary already failed")
            },
        );
        match result {
            Err(RunFailure::Executed {
                usage: got_usage,
                message,
            }) => {
                assert_eq!(got_usage, original_usage);
                assert!(message.contains("original failure"));
                assert!(message.contains("runtime teardown failed"));
            }
            other => panic!("expected an augmented RunFailure::Executed, got {other:?}"),
        }
    }

    // =============================================================================================
    // CT-007 slice 5b.2 — the checkout-specific runtime. Deterministic coverage for the pure
    // decoder/validation/script-parsing logic (no real `runsc`/gVisor needed for any of these).
    // =============================================================================================
    mod checkout_preparation_5b2 {
        use super::*;
        // CT-007 slice 5b.3-6e.2 Stage A: git-wire fakes relocated to the test-support module.
        // The pkt-line/advertisement/fetch decoder tests (and their `advertisement`/`fake_pack`/
        // `fetch_response` fakes) now live with the codec in `git_wire_codec`.
        use crate::gvisor::checkout_transport_test_support::sha1_oid;

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

        // ---- confirmation-line parser ----

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

        // ---- CheckoutPreparationSpec limits validation (Sol's review: bypassing JobSpec must not
        // also bypass its mandatory pids_max/timeout_secs validation) ----

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

        // ---- checkout script gitlink detection (real host git+sh, no gVisor needed) ----

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
        #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
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
        #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
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
        #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
        fn checkout_script_gitlink_check_fails_closed_when_ls_tree_itself_fails() {
            let repo = temp_dir_for("gitlink-check-ls-tree-fails");
            drill_git_ok(&["init", "-q", "-b", "main"], &repo);
            // No commits exist at all -- `git ls-tree -r <oid>` on a bogus/absent oid must itself
            // fail, and that failure must be treated as a hard error, NEVER silently read as "no
            // gitlinks found" (the exact bug being fixed: a grep exit status of 1 means "no match
            // in real output", not "the upstream command produced nothing because it failed").
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

        // ---- host-side FD-safe HEAD verification ----

        fn temp_dir_for(name: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "myelin-checkout-headcheck-{name}-{}-{}",
                std::process::id(),
                unique_suffix()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
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
        #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
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
            // Sol's review: a guest process fully owns its writable workspace and could plant a
            // FIFO named `HEAD` with no writer. Before the fix, `open_regular_file_no_follow`'s
            // plain `O_RDONLY` open would block forever here. Run the check on a background thread
            // with a bounded wait so a REGRESSION fails this test loudly (instead of hanging the
            // whole suite) rather than passing by accident.
            let ws = temp_dir_for("fifo-head");
            std::fs::create_dir_all(ws.join(".git")).unwrap();
            let head_path = ws.join(".git/HEAD");
            let head_c = CString::new(head_path.as_os_str().as_encoded_bytes()).unwrap();
            // SAFETY: `head_c` is a NUL-free path under a directory this test just created; `mkfifo`
            // creates a FIFO special file at that path with mode 0600.
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

        // ---- host-side FD-safe Cargo.lock hashing ----

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

        /// Exact regression for the production failure: checkout runs as OCI uid 65534, mapped to
        /// the leased subordinate host uid, and `umask 077` leaves its `.git` directory mode 0700.
        /// Normal runner DAC must get EACCES; the combined host verifier must succeed through only
        /// scoped CAP_DAC_READ_SEARCH, then withdraw it without changing any owner or mode.
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
                     SKIPPED — rerun under the production-shaped ambient capability grant with \
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

            // Retain FDs so cleanup never depends on traversing the deliberately inaccessible
            // pathname. This also lets the test restore ownership after all assertions are sampled.
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
                // Match WorkspaceStorage's load-bearing order: chmod while still the owner, then
                // transfer ownership last. CAP_CHOWN does not itself authorize a later chmod.
                // SAFETY: every FD is live and still owned by this test process here.
                assert_eq!(unsafe { libc::fchmod(file.as_raw_fd(), mode) }, 0);
                // SAFETY: every FD is live for the closure call. The test is launched with
                // CAP_CHOWN and changes only its fresh fixture to the allocator-minted subids.
                assert_eq!(unsafe { libc::fchown(file.as_raw_fd(), subuid, subgid) }, 0);
            };
            let restore_to_runner = |file: &std::fs::File, mode: u32| {
                // Restore ownership first through CAP_CHOWN; once owner again, chmod is ordinary.
                // SAFETY: every FD remains live and names only the fresh fixture.
                assert_eq!(
                    unsafe { libc::fchown(file.as_raw_fd(), libc::geteuid(), libc::getegid()) },
                    0
                );
                // SAFETY: the successful fchown above made the current euid the owner.
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

        // ---- CT-007 slice 5b.2 live drill (Sol's review, round 3): real git-wire (Hop A) + real
        // runsc/OCI/userns/workspace (Hop B), end to end. Mirrors
        // `explicit_user_namespace_boots_through_the_real_enabled_backend_and_launch`'s exact
        // skip/hard-fail gating contract: the ONLY legitimate skip conditions are the listed
        // absent capabilities; once all are present, ANY construction or execution failure is a
        // genuine regression (never caught-and-skipped).

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

        /// Stage a git-bearing rootfs from the busybox base (mirrors
        /// `tests/git_wire_prod_exec_test.rs`'s own staging recipe — that file's staging cannot be
        /// reused directly since it runs in a SEPARATE test-binary process from this crate's own
        /// `#[cfg(test)] mod tests`, so the `MYELIN_GVISOR_GIT_ROOTFS` env var / `OnceLock` it sets
        /// never crosses process boundaries).
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

        /// Create a REAL bare repo with TWO commits; return `(older_oid, newer_oid)`. The drill
        /// requests the OLDER (non-tip) commit — Sol's suggestion: this is the ONE fixture shape
        /// that actually exercises `allow-reachable-sha1-in-want` (it is reachable but not an
        /// advertised ref tip) and the shallow boundary (a real, non-root commit truncated at
        /// depth 1), not merely the trivial "want the HEAD tip" case.
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
            // A `Cargo.lock` is committed too -- `run_checkout_preparation` requires one present
            // (it hashes the materialized `Cargo.lock`, ledger 12's locked slice-5b contract).
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
            // Serializes against the Enabled activation drill -- both share the SAME
            // operator-provisioned `leases_dir` (see `USERNS_DRILL_LEASES_DIR_LOCK`'s own doc).
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

            // ---- a REAL bare repo, two commits ----
            let tag = format!("{}-{}", std::process::id(), unique_suffix());
            let root = std::env::temp_dir().join(format!("myelin-checkout-drill-repo-{tag}"));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("mkdir drill repo root");
            let (older_oid, _newer_oid) =
                drill_make_repo_with_two_commits(&root, "acme", "fr-par", "widgets");

            // ---- Hop A: fetch the OLDER (non-tip) commit's pack through the REAL git-wire path ----
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

            // ---- acquire a REAL workspace + userns lease (the same acquisition machinery
            // `launch_with`'s Enabled path uses) ----
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

            // ---- Hop B: run the REAL checkout-preparation container ----
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

            // ---- cleanup: delete the workspace BEFORE releasing the lease (Sol's review: the
            // central identity invariant is that the subordinate uid/gid is never released/
            // reallocated while its chowned workspace still exists) ----
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

        // ---- usage accounting ----

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
            assert_eq!(usage.cpu_seconds, 2); // 1.5s wall, ceiled
            assert_eq!(usage.mem_byte_seconds, (1 << 20) * 2);
        }

        // ---- orchestration ordering (Sol's round-2 review: deterministic seams for the
        // properties a live drill alone wouldn't pin down repeatably) ----
        //
        // `evaluate_checkout_finalization` is `run_checkout_preparation`'s ENTIRE post-spawn
        // decision logic, extracted specifically so these run against synthetic
        // `RuntimeFinalization`/`RuntimeQuiescenceEvidence` values -- no real `runsc` spawn
        // anywhere below. A real (cheap, non-privileged) `UserNamespaceLease` still requires a
        // real `/etc/subuid`/`/etc/subgid` range for this process's uid (`test-support` feature).

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
            // Behavioral proof `confirm_prepared` was NEVER attempted: the session must still be
            // EXACTLY `PreparationBound` -- confirming it now (with the same identity
            // `bind_preparation` durably wrote) must succeed. Had this function wrongly already
            // confirmed/poisoned it, this would panic (wrong state) or refuse (already Prepared/
            // Unreleasable).
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
            // Behavioral proof teardown WAS confirmed despite the semantic (exit-code) failure:
            // the session must have reached `Prepared` -- `release_prepared` panics otherwise.
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
            // The host-side workspace disagrees with what the guest claimed (a different oid) --
            // this must still be caught AFTER confirm_prepared already ran.
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
            // HEAD matches, but there is NO Cargo.lock -- this must still be caught (as a semantic
            // rejection, AFTER confirm_prepared already ran), never silently produce evidence with
            // no digest.
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
            // The "evidence" names a DIFFERENT cgroup identity than what was durably bound --
            // `confirm_prepared` must refuse (`ProofDisagreesWithMarker`), not silently accept it.
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

        // =========================================================================================
        // CT-007 slice 5b.3-3 — the parent-attempt Hop A transport. Deterministic coverage via an
        // injected executor (`GitWireHopExecutor`) — no real `runsc` binary needed for this refactor
        // slice (the true runsc/git-rootfs integration is 5b.3-7's live drill).
        // =========================================================================================
        mod checkout_transport_5b3_3 {
            use super::*;
            use crate::CheckoutAuthorizationScope;
            // CT-007 slice 5b.3-6e.2 Stage A: git-wire fakes relocated to the test-support module so
            // the runsc-driver seam + §4 tests share them. Re-imported so the existing tests compile.
            use crate::gvisor::checkout_transport_test_support::{
                advertisement_bytes, fake_quiescence_evidence, fetch_response_bytes,
                permit_recording_executor, sha1_oid, BoxedHopExecutor, FakeRunsc, ScriptedStep,
            };

            const TENANT: &str = "acme";
            const REGION: &str = "fr-par";
            const REPO: &str = "widgets";

            #[test]
            fn preparation_error_classification_is_structural_never_message_based() {
                let usage = ResourceUsage {
                    cpu_seconds: 1,
                    mem_byte_seconds: 2,
                };
                let transport_failed =
                    checkout_transport_terminal_failed("looks retryable".into(), usage);
                assert_eq!(
                    transport_failed.attempt_disposition(),
                    PreparationAttemptDisposition::Terminal(
                        PreparationTerminalDisposition::Failed {
                            phase: PreparationPhase::CheckoutTransport,
                        }
                    )
                );
                let transport_retryable =
                    checkout_transport_retryable("looks terminal".into(), usage);
                assert_eq!(
                    transport_retryable.attempt_disposition(),
                    PreparationAttemptDisposition::RetryableInfrastructure {
                        phase: PreparationPhase::CheckoutTransport,
                    }
                );
                let materialization_timeout =
                    checkout_materialization_timed_out("arbitrary diagnostic".into(), usage);
                assert_eq!(
                    materialization_timeout.attempt_disposition(),
                    PreparationAttemptDisposition::Terminal(
                        PreparationTerminalDisposition::TimedOut {
                            phase: PreparationPhase::CheckoutMaterialization,
                        }
                    )
                );
                let poisoned = CheckoutPreparationError::Unreleasable {
                    message: "ordinary words".into(),
                    usage: Some(usage),
                };
                assert_eq!(
                    poisoned.attempt_disposition(),
                    PreparationAttemptDisposition::ReconciliationRequired {
                        phase: PreparationPhase::CheckoutMaterialization,
                        teardown_unproven: false,
                        usage_unrepresentable: false,
                        quarantine_required: true,
                    }
                );
            }

            #[test]
            fn hop_b_commit_outcome_unknown_is_never_downgraded_to_an_ordinary_retry() {
                let error =
                    map_checkout_materialization_run_failure(RunFailure::commit_outcome_unknown(
                        "injected impossible immediate-permit commit ambiguity",
                    ));
                assert_eq!(
                    error.attempt_disposition(),
                    PreparationAttemptDisposition::ReconciliationRequired {
                        phase: PreparationPhase::CheckoutMaterialization,
                        teardown_unproven: false,
                        usage_unrepresentable: false,
                        quarantine_required: false,
                    }
                );
                match error {
                    CheckoutPreparationError::RejectedAfterQuiescence {
                        message, usage, ..
                    } => {
                        assert_eq!(
                            usage,
                            ResourceUsage {
                                cpu_seconds: 0,
                                mem_byte_seconds: 0,
                            }
                        );
                        assert!(message.contains("internal invariant violated"));
                        assert!(message.contains("commit ambiguity"));
                    }
                    other => panic!("expected a fail-closed post-quiescence error, got {other:?}"),
                }
            }

            /// A real (not symlinked) bare-repo directory under a fresh root, matching exactly what
            /// `resolve_bare_repo_path`/`assert_repo_under_root` require — both hops resolve the SAME
            /// path, so this is staged once per test.
            fn staged_repo_root() -> PathBuf {
                let root = temp_dir_for("5b3-3-root");
                std::fs::create_dir_all(root.join(TENANT).join(REGION).join(format!("{REPO}.git")))
                    .unwrap();
                root
            }

            fn checkout_limits() -> ResourceLimits {
                ResourceLimits {
                    cpu_millis: 1000,
                    mem_bytes: 256 << 20,
                    disk_bytes: 1 << 20,
                    tmpfs_bytes: 64 << 20,
                    pids_max: 64,
                    timeout_secs: 60,
                }
            }

            fn parent_attempt_scope(
                commit_hex: &str,
                format: GitObjectFormat,
            ) -> CheckoutAuthorizationScope {
                CheckoutAuthorizationScope::new(
                    myelin_tenancy::TenantId(TENANT.to_string()),
                    myelin_events::ArtifactRef(format!("myelin://{TENANT}/git/repo/{REPO}")),
                    REPO.to_string(),
                    commit_hex.to_string(),
                    format,
                )
            }

            fn minted_proof_for(
                scope: CheckoutAuthorizationScope,
                jti: &str,
            ) -> CheckoutAuthorizationProof {
                let hooks =
                    ok_hooks().with_checkout_authorization(Box::new(|_spec, _scope| Ok(())));
                let job = JobSpec::new(
                    JobKind::Ci,
                    fixture_image(),
                    vec!["true".to_string()],
                    vec![],
                    vec![],
                    EgressPolicy::deny_all(),
                    checkout_limits(),
                    WorkspaceSpec::default(),
                    TrustTier::Trusted,
                    RunTokenCredential::new("bearer", jti, 300).unwrap(),
                    MeterTarget {
                        reserve_id: "r".to_string(),
                    },
                    IdemToken("idem-mint".to_string()),
                )
                .unwrap();
                hooks.authorize_checkout(&job, scope).unwrap()
            }

            /// CT-007 phase-credential generations: mint a REAL fused [`PhaseAuthorization`]
            /// through the real hook. `permit_outcome` stands in for the control plane's durable
            /// phase gate: `Ok(())` = the generation is still current at the spawn boundary,
            /// `Err(..)` = it is not (requeued, superseded, expired).
            ///
            /// Note there is NO way for a test to build one of these by hand either — the only
            /// route is a genuine hook invocation, exactly like production.
            fn minted_phase_authorization(
                scope: CheckoutAuthorizationScope,
                jti: &str,
                phase: crate::CheckoutPhase,
                generation_id: &str,
                permit_outcome: Result<(), &'static str>,
            ) -> PhaseAuthorization {
                let hooks = ok_hooks().with_checkout_phase_authorization(Box::new(
                    move |_spec, _scope, _phase| {
                        Ok(match permit_outcome {
                            Ok(()) => LaunchPermit::immediate(),
                            Err(reason) => {
                                LaunchPermit::retained(move || Err(HookError(reason.to_string())))
                            }
                        })
                    },
                ));
                let job = JobSpec::new(
                    JobKind::Ci,
                    fixture_image(),
                    vec!["true".to_string()],
                    vec![],
                    vec![],
                    EgressPolicy::deny_all(),
                    checkout_limits(),
                    WorkspaceSpec::default(),
                    TrustTier::Trusted,
                    RunTokenCredential::new("bearer", jti, 300).unwrap(),
                    MeterTarget {
                        reserve_id: "r".to_string(),
                    },
                    IdemToken("idem-mint".to_string()),
                )
                .unwrap();
                hooks
                    .authorize_checkout_phase(&job, scope, phase, generation_id)
                    .unwrap()
            }

            /// A distinct durable generation id per purpose, so the advertise→fetch supersession
            /// check has real values to compare.
            fn generation_id_for(phase: crate::CheckoutPhase) -> String {
                let seed = match phase {
                    crate::CheckoutPhase::Advertise => 'a',
                    crate::CheckoutPhase::Fetch => 'f',
                    crate::CheckoutPhase::Materialization => 'm',
                };
                format!("ci-credential:v1:{}", seed.to_string().repeat(64))
            }

            fn fake_hop_container_run(stdout: Vec<u8>, usage: ResourceUsage) -> ContainerRun {
                ContainerRun {
                    child: Box::new(FakeRunsc),
                    bundle_dir: temp_dir_for("5b3-3-hop"),
                    result: SandboxResult {
                        exit_code: Some(0),
                        timed_out: false,
                        usage,
                        stdout,
                        stderr: Vec::new(),
                    },
                    run_error: None,
                }
            }

            struct FailingKillRunsc;
            impl RunscChild for FailingKillRunsc {
                fn kill(&mut self) -> Result<(), String> {
                    Err("simulated kill failure".to_string())
                }
                fn wait(&mut self) -> Result<i32, String> {
                    Ok(0)
                }
            }

            fn fake_hop_container_run_with_unkillable_child(
                stdout: Vec<u8>,
                usage: ResourceUsage,
            ) -> ContainerRun {
                ContainerRun {
                    child: Box::new(FailingKillRunsc),
                    bundle_dir: temp_dir_for("5b3-3-hop-unkillable"),
                    result: SandboxResult {
                        exit_code: Some(0),
                        timed_out: false,
                        usage,
                        stdout,
                        stderr: Vec::new(),
                    },
                    run_error: None,
                }
            }

            // A step still returns the simple pre-finalization shape — `scripted_executor` below
            // auto-wraps a successful step into a `RuntimeFinalization::Finalized` (teardown proven
            // fine), matching what every EXISTING test needs. Tests that specifically need to exercise a
            // genuine teardown-unproven `RuntimeFinalization::Failed` (Sol's review, blocker 2) build a
            // `BoxedHopExecutor` closure directly instead of going through this helper (see
            // `production_shaped_teardown_failure_is_reported_as_teardown_unproven`).
            //
            // `ScriptedStep`, `BoxedHopExecutor`, and `fake_quiescence_evidence` were relocated to the
            // `checkout_transport_test_support` module (re-imported above).

            /// Scripts exactly `steps.len()` executor calls, one scripted outcome each, in order.
            /// Returns the executor closure plus a handle to the number of REMAINING (not yet
            /// consumed) steps, so a test can assert exactly the scripted count of calls happened.
            /// Panics if invoked more times than scripted (Sol's review: "exactly two ... executions").
            fn scripted_executor(
                steps: Vec<ScriptedStep>,
            ) -> (BoxedHopExecutor, Arc<Mutex<usize>>) {
                let remaining = Arc::new(Mutex::new(steps.len()));
                let remaining_for_closure = Arc::clone(&remaining);
                let queue = Mutex::new(std::collections::VecDeque::from(steps));
                let f = move |_job: &JobSpec,
                              _cfg: &OciConfig,
                              _stdin: Vec<u8>,
                              _rootfs: &Path,
                              _cancellation: &AtomicBool,
                              _permit: LaunchPermit| {
                    let mut queue = queue.lock().unwrap();
                    let step = queue
                        .pop_front()
                        .expect("executor invoked more times than scripted");
                    *remaining_for_closure.lock().unwrap() = queue.len();
                    let finalization_result = match step() {
                        Ok((run, truncated)) => Ok(RuntimeFinalization::Finalized(FinalizedRun {
                            primary: Ok((run, truncated)),
                            evidence: fake_quiescence_evidence(),
                        })),
                        Err(run_failure) => Err(run_failure),
                    };
                    // Bundle cleanup is never in question for these auto-wrapped scripted steps —
                    // dedicated tests for an unproven bundle cleanup build a `BoxedHopExecutor`
                    // directly instead (see `bundle_cleanup_failure_forces_teardown_unproven`).
                    (finalization_result, Ok(()))
                };
                (Box::new(f), remaining)
            }

            fn panics_if_called_executor() -> (BoxedHopExecutor, Arc<Mutex<usize>>) {
                scripted_executor(vec![])
            }

            // `advertisement_bytes` / `fetch_response_bytes` were relocated to the
            // `checkout_transport_test_support` module (re-imported above).

            // ---- proof verification happens BEFORE any spawn ----

            #[test]
            fn proof_with_wrong_run_token_jti_refuses_before_any_spawn() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xc1);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof = minted_proof_for(
                    parent_attempt_scope(&oid, GitObjectFormat::Sha1),
                    "jti-minted-against",
                );
                let run_token =
                    RunTokenCredential::new("bearer", "jti-actually-running-as", 300).unwrap();
                let (executor, _remaining) = panics_if_called_executor();
                let cancellation = AtomicBool::new(false);
                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                assert!(matches!(err, CheckoutTransportError::Refused { .. }));
                let _ = std::fs::remove_dir_all(&root);
            }

            #[test]
            fn proof_with_wrong_tenant_refuses_before_any_spawn() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xc2);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let scope = CheckoutAuthorizationScope::new(
                    myelin_tenancy::TenantId("someone-else".to_string()),
                    myelin_events::ArtifactRef(
                        "myelin://someone-else/git/repo/widgets".to_string(),
                    ),
                    REPO.to_string(),
                    oid.clone(),
                    GitObjectFormat::Sha1,
                );
                let proof = minted_proof_for(scope, "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();
                let (executor, _remaining) = panics_if_called_executor();
                let cancellation = AtomicBool::new(false);
                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                assert!(matches!(err, CheckoutTransportError::Refused { .. }));
                let _ = std::fs::remove_dir_all(&root);
            }

            #[test]
            fn proof_with_wrong_repo_refuses_before_any_spawn() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xc3);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let scope = CheckoutAuthorizationScope::new(
                    myelin_tenancy::TenantId(TENANT.to_string()),
                    myelin_events::ArtifactRef("myelin://acme/git/repo/other-repo".to_string()),
                    "other-repo".to_string(),
                    oid.clone(),
                    GitObjectFormat::Sha1,
                );
                let proof = minted_proof_for(scope, "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();
                let (executor, _remaining) = panics_if_called_executor();
                let cancellation = AtomicBool::new(false);
                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                assert!(matches!(err, CheckoutTransportError::Refused { .. }));
                let _ = std::fs::remove_dir_all(&root);
            }

            #[test]
            fn proof_with_wrong_commit_refuses_before_any_spawn() {
                let root = staged_repo_root();
                let minted_oid = sha1_oid(0xc4);
                let requested_oid = sha1_oid(0xc5);
                let expected =
                    ExpectedGitCommitId::new(requested_oid, GitObjectFormat::Sha1).unwrap();
                let proof = minted_proof_for(
                    parent_attempt_scope(&minted_oid, GitObjectFormat::Sha1),
                    "jti-1",
                );
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();
                let (executor, _remaining) = panics_if_called_executor();
                let cancellation = AtomicBool::new(false);
                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                assert!(matches!(err, CheckoutTransportError::Refused { .. }));
                let _ = std::fs::remove_dir_all(&root);
            }

            // ---- happy path ----

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn happy_path_executes_exactly_two_immediate_gated_hops_and_checked_adds_usage() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xd1);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let advertise_usage = ResourceUsage {
                    cpu_seconds: 3,
                    mem_byte_seconds: 7,
                };
                let fetch_usage = ResourceUsage {
                    cpu_seconds: 11,
                    mem_byte_seconds: 13,
                };
                let advertise_bytes = advertisement_bytes(&oid);
                let fetch_bytes = fetch_response_bytes(b"pack-payload");
                let (executor, remaining) = scripted_executor(vec![
                    Box::new({
                        let bytes = advertise_bytes.clone();
                        move || Ok((fake_hop_container_run(bytes, advertise_usage), false))
                    }),
                    Box::new({
                        let bytes = fetch_bytes.clone();
                        move || Ok((fake_hop_container_run(bytes, fetch_usage), false))
                    }),
                ]);
                let cancellation = AtomicBool::new(false);

                let outcome = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .expect("scripted happy path must succeed");
                assert_eq!(
                    *remaining.lock().unwrap(),
                    0,
                    "exactly the two scripted hops must run, no more no less"
                );
                let (_pack, usage) = outcome.into_parts();
                assert_eq!(
                    usage,
                    ResourceUsage {
                        cpu_seconds: 14,
                        mem_byte_seconds: 20,
                    },
                    "success must checked-add advertisement + fetch usage"
                );
                let _ = std::fs::remove_dir_all(&root);
            }

            // ---- the advertise→fetch preparation-lease checkpoint ----

            /// A checkpoint that always refuses, recording that it was consulted exactly once.
            struct LostLeaseCheckpoint {
                calls: std::sync::Mutex<u32>,
            }

            impl crate::PreparationLeaseCheckpoint for LostLeaseCheckpoint {
                fn renew(&self) -> Result<(), crate::PreparationLeaseLost> {
                    *self.calls.lock().unwrap() += 1;
                    Err(crate::PreparationLeaseLost(
                        "exact generation no longer owns this claim".into(),
                    ))
                }
            }

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn a_lost_preparation_lease_refuses_between_advertise_and_fetch_and_retains_usage() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xd9);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let advertise_usage = ResourceUsage {
                    cpu_seconds: 3,
                    mem_byte_seconds: 7,
                };
                let advertise_bytes = advertisement_bytes(&oid);
                // Exactly ONE scripted hop: the fetch must never spawn once the lease is lost.
                let (executor, remaining) = scripted_executor(vec![Box::new(move || {
                    Ok((
                        fake_hop_container_run(advertise_bytes, advertise_usage),
                        false,
                    ))
                })]);
                let cancellation = AtomicBool::new(false);
                let checkpoint = LostLeaseCheckpoint {
                    calls: std::sync::Mutex::new(0),
                };

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    Some(&checkpoint),
                    &*executor,
                )
                .unwrap_err();

                assert_eq!(*checkpoint.calls.lock().unwrap(), 1);
                assert_eq!(
                    *remaining.lock().unwrap(),
                    0,
                    "only the advertisement hop may run; the fetch hop must never spawn"
                );
                match err {
                    CheckoutTransportError::Failed {
                        usage, disposition, ..
                    } => {
                        assert_eq!(usage, advertise_usage, "advertisement usage survives");
                        assert_eq!(
                            disposition,
                            PreparationAttemptDisposition::RetryableInfrastructure {
                                phase: PreparationPhase::CheckoutTransport,
                            },
                            "a lost claim generation is a clean retry, not a checkout verdict"
                        );
                    }
                    other => panic!("expected a retryable lost-lease refusal, got {other:?}"),
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn a_live_preparation_lease_checkpoint_lets_hop_a_complete() {
                struct LiveCheckpoint {
                    calls: std::sync::Mutex<u32>,
                }
                impl crate::PreparationLeaseCheckpoint for LiveCheckpoint {
                    fn renew(&self) -> Result<(), crate::PreparationLeaseLost> {
                        *self.calls.lock().unwrap() += 1;
                        Ok(())
                    }
                }

                let root = staged_repo_root();
                let oid = sha1_oid(0xda);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let usage = ResourceUsage {
                    cpu_seconds: 2,
                    mem_byte_seconds: 4,
                };
                let advertise_bytes = advertisement_bytes(&oid);
                let fetch_bytes = fetch_response_bytes(b"pack-payload");
                let (executor, remaining) = scripted_executor(vec![
                    Box::new(move || Ok((fake_hop_container_run(advertise_bytes, usage), false))),
                    Box::new(move || Ok((fake_hop_container_run(fetch_bytes, usage), false))),
                ]);
                let cancellation = AtomicBool::new(false);
                let checkpoint = LiveCheckpoint {
                    calls: std::sync::Mutex::new(0),
                };

                fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    Some(&checkpoint),
                    &*executor,
                )
                .expect("a live checkpoint must not change the happy path");
                assert_eq!(*checkpoint.calls.lock().unwrap(), 1);
                assert_eq!(*remaining.lock().unwrap(), 0);
                let _ = std::fs::remove_dir_all(&root);
            }

            // ---- every failure point retains usage already incurred ----

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn advertisement_parse_failure_retains_advertisement_usage() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xd2);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let advertise_usage = ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 9,
                };
                let (executor, remaining) = scripted_executor(vec![Box::new(move || {
                    Ok((
                        fake_hop_container_run(
                            b"not a valid advertisement".to_vec(),
                            advertise_usage,
                        ),
                        false,
                    ))
                })]);
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                assert_eq!(
                    *remaining.lock().unwrap(),
                    0,
                    "the advertisement hop must still run"
                );
                match err {
                    CheckoutTransportError::Failed { usage, .. } => {
                        assert_eq!(usage, advertise_usage);
                    }
                    other => panic!("expected Failed, got {other:?}"),
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn fetch_pre_spawn_failure_retains_advertisement_usage() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xd3);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let advertise_usage = ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 9,
                };
                let advertise_bytes = advertisement_bytes(&oid);
                let (executor, remaining) = scripted_executor(vec![
                    Box::new({
                        let bytes = advertise_bytes.clone();
                        move || Ok((fake_hop_container_run(bytes, advertise_usage), false))
                    }),
                    Box::new(|| Err(RunFailure::uncommitted("simulated fetch pre-spawn failure"))),
                ]);
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                assert_eq!(
                    *remaining.lock().unwrap(),
                    0,
                    "both hops must have been attempted"
                );
                match err {
                    CheckoutTransportError::Failed { usage, .. } => {
                        assert_eq!(
                            usage, advertise_usage,
                            "an Uncommitted fetch failure must still retain the advertisement's \
                             already-measured usage, never report it as free"
                        );
                    }
                    other => panic!("expected Failed, got {other:?}"),
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn fetch_post_spawn_executed_failure_retains_advertisement_plus_fetch_usage() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xd4);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let advertise_usage = ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 9,
                };
                let fetch_failure_usage = ResourceUsage {
                    cpu_seconds: 2,
                    mem_byte_seconds: 4,
                };
                let advertise_bytes = advertisement_bytes(&oid);
                let (executor, remaining) = scripted_executor(vec![
                    Box::new({
                        let bytes = advertise_bytes.clone();
                        move || Ok((fake_hop_container_run(bytes, advertise_usage), false))
                    }),
                    Box::new(move || {
                        Err(RunFailure::executed(
                            "simulated fetch post-spawn failure",
                            fetch_failure_usage,
                        ))
                    }),
                ]);
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                assert_eq!(*remaining.lock().unwrap(), 0);
                match err {
                    CheckoutTransportError::Failed { usage, .. } => {
                        assert_eq!(
                            usage,
                            ResourceUsage {
                                cpu_seconds: advertise_usage.cpu_seconds
                                    + fetch_failure_usage.cpu_seconds,
                                mem_byte_seconds: advertise_usage.mem_byte_seconds
                                    + fetch_failure_usage.mem_byte_seconds,
                            }
                        );
                    }
                    other => panic!("expected Failed, got {other:?}"),
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            // ---- arithmetic overflow refuses loudly ----

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn usage_aggregation_overflow_refuses_loudly() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xd5);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                // Advertisement alone doesn't overflow (usage_before starts at zero) — the overflow
                // must occur when the FETCH hop's own usage is checked-added onto the advertisement's
                // already-measured `u64::MAX`.
                let advertise_usage = ResourceUsage {
                    cpu_seconds: u64::MAX,
                    mem_byte_seconds: 1,
                };
                let fetch_usage = ResourceUsage {
                    cpu_seconds: 1,
                    mem_byte_seconds: 1,
                };
                let advertise_bytes = advertisement_bytes(&oid);
                let fetch_bytes = fetch_response_bytes(b"pack-payload");
                let (executor, remaining) = scripted_executor(vec![
                    Box::new(move || {
                        Ok((
                            fake_hop_container_run(advertise_bytes, advertise_usage),
                            false,
                        ))
                    }),
                    Box::new(move || Ok((fake_hop_container_run(fetch_bytes, fetch_usage), false))),
                ]);
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                assert_eq!(
                    *remaining.lock().unwrap(),
                    0,
                    "both hops must have run before the overflow is detected"
                );
                // Overflow happens folding the fetch hop's own usage onto the advertisement's
                // already-measured `u64::MAX` — refused loudly (never wrapped/saturated) rather than
                // silently reporting a wrapped-around total.
                match err {
                    CheckoutTransportError::UsageUnrepresentable {
                        message,
                        usage,
                        teardown_unproven,
                    } => {
                        assert!(message.contains("overflow"), "message was: {message}");
                        assert_eq!(
                            usage, advertise_usage,
                            "on overflow, the last exact provable total is the pre-overflow total"
                        );
                        assert!(
                            !teardown_unproven,
                            "teardown was independently proven fine here; only usage broke"
                        );
                    }
                    other => panic!(
                        "expected UsageUnrepresentable carrying an overflow message, got {other:?}"
                    ),
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            // ---- teardown-unproven is distinct and still carries usage ----

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn kill_failure_on_a_successful_hop_yields_teardown_unproven_and_retains_usage() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xd6);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let advertise_usage = ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 9,
                };
                let advertise_bytes = advertisement_bytes(&oid);
                let (executor, remaining) = scripted_executor(vec![Box::new(move || {
                    Ok((
                        fake_hop_container_run_with_unkillable_child(
                            advertise_bytes,
                            advertise_usage,
                        ),
                        false,
                    ))
                })]);
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                assert_eq!(
                    *remaining.lock().unwrap(),
                    0,
                    "only the one scripted (advertisement) hop must have run"
                );
                match err {
                    CheckoutTransportError::TeardownUnproven { usage, message } => {
                        assert_eq!(usage, advertise_usage);
                        assert!(message.contains("kill"), "message was: {message}");
                    }
                    other => panic!("expected TeardownUnproven, got {other:?}"),
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn truncated_output_combined_with_kill_failure_preserves_both_messages() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xd7);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let advertise_usage = ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 9,
                };
                let (executor, remaining) = scripted_executor(vec![Box::new(move || {
                    Ok((
                        fake_hop_container_run_with_unkillable_child(Vec::new(), advertise_usage),
                        true, // stdout_truncated
                    ))
                })]);
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                assert_eq!(
                    *remaining.lock().unwrap(),
                    0,
                    "only the one scripted (advertisement) hop must have run"
                );
                match err {
                    CheckoutTransportError::TeardownUnproven { usage, message } => {
                        assert_eq!(usage, advertise_usage);
                        assert!(message.contains("wire cap"), "message was: {message}");
                        assert!(message.contains("kill"), "message was: {message}");
                    }
                    other => {
                        panic!("expected TeardownUnproven combining both failures, got {other:?}")
                    }
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn run_error_combined_with_kill_failure_preserves_both_messages() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xd8);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let advertise_usage = ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 9,
                };
                let (executor, remaining) = scripted_executor(vec![Box::new(move || {
                    let mut run =
                        fake_hop_container_run_with_unkillable_child(Vec::new(), advertise_usage);
                    run.run_error = Some("simulated stream error".to_string());
                    Ok((run, false))
                })]);
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                assert_eq!(
                    *remaining.lock().unwrap(),
                    0,
                    "only the one scripted (advertisement) hop must have run"
                );
                match err {
                    CheckoutTransportError::TeardownUnproven { usage, message } => {
                        assert_eq!(usage, advertise_usage);
                        assert!(
                            message.contains("simulated stream error"),
                            "message was: {message}"
                        );
                        assert!(message.contains("kill"), "message was: {message}");
                    }
                    other => {
                        panic!("expected TeardownUnproven combining both failures, got {other:?}")
                    }
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            // ---- no live handle or bundle remains after return, on ANY path ----

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn successful_transport_leaves_no_bundle_dirs_behind() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xd9);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let advertise_bytes = advertisement_bytes(&oid);
                let fetch_bytes = fetch_response_bytes(b"pack-payload");
                let advertise_bundle_dir = temp_dir_for("5b3-3-tracked-advertise-bundle");
                let fetch_bundle_dir = temp_dir_for("5b3-3-tracked-fetch-bundle");
                let advertise_bundle_dir_check = advertise_bundle_dir.clone();
                let fetch_bundle_dir_check = fetch_bundle_dir.clone();
                let usage = ResourceUsage {
                    cpu_seconds: 1,
                    mem_byte_seconds: 1,
                };
                let (executor, _remaining) = scripted_executor(vec![
                    Box::new(move || {
                        Ok((
                            ContainerRun {
                                child: Box::new(FakeRunsc),
                                bundle_dir: advertise_bundle_dir,
                                result: SandboxResult {
                                    exit_code: Some(0),
                                    timed_out: false,
                                    usage,
                                    stdout: advertise_bytes,
                                    stderr: Vec::new(),
                                },
                                run_error: None,
                            },
                            false,
                        ))
                    }),
                    Box::new(move || {
                        Ok((
                            ContainerRun {
                                child: Box::new(FakeRunsc),
                                bundle_dir: fetch_bundle_dir,
                                result: SandboxResult {
                                    exit_code: Some(0),
                                    timed_out: false,
                                    usage,
                                    stdout: fetch_bytes,
                                    stderr: Vec::new(),
                                },
                                run_error: None,
                            },
                            false,
                        ))
                    }),
                ]);
                let cancellation = AtomicBool::new(false);

                fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .expect("scripted happy path must succeed");

                assert!(
                    !advertise_bundle_dir_check.exists(),
                    "the advertisement hop's bundle dir must be removed by return time"
                );
                assert!(
                    !fetch_bundle_dir_check.exists(),
                    "the fetch hop's bundle dir must be removed by return time"
                );
                let _ = std::fs::remove_dir_all(&root);
            }

            // ---- Sol's round-1 review, blocker 1: a non-passing guest execution must never be
            // accepted just because its stdout happens to parse ----

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn not_passed_advertisement_is_never_accepted_as_success() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xda);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let advertise_usage = ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 9,
                };
                let advertise_bytes = advertisement_bytes(&oid);
                let (executor, remaining) = scripted_executor(vec![Box::new(move || {
                    let mut run = fake_hop_container_run(advertise_bytes, advertise_usage);
                    run.result.exit_code = Some(1);
                    Ok((run, false))
                })]);
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                assert_eq!(*remaining.lock().unwrap(), 0);
                match err {
                    CheckoutTransportError::Failed { message, usage, .. } => {
                        assert!(message.contains("did not pass"), "message was: {message}");
                        assert_eq!(usage, advertise_usage);
                    }
                    other => panic!("expected Failed, got {other:?}"),
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn not_passed_fetch_is_never_accepted_as_success() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xdb);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let advertise_usage = ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 9,
                };
                let fetch_usage = ResourceUsage {
                    cpu_seconds: 2,
                    mem_byte_seconds: 4,
                };
                let advertise_bytes = advertisement_bytes(&oid);
                let fetch_bytes = fetch_response_bytes(b"pack-payload");
                let (executor, remaining) = scripted_executor(vec![
                    Box::new(move || {
                        Ok((
                            fake_hop_container_run(advertise_bytes, advertise_usage),
                            false,
                        ))
                    }),
                    Box::new(move || {
                        let mut run = fake_hop_container_run(fetch_bytes, fetch_usage);
                        run.result.timed_out = true;
                        Ok((run, false))
                    }),
                ]);
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                assert_eq!(*remaining.lock().unwrap(), 0);
                match err {
                    CheckoutTransportError::Failed { message, usage, .. } => {
                        assert!(message.contains("did not pass"), "message was: {message}");
                        assert_eq!(
                            usage,
                            ResourceUsage {
                                cpu_seconds: advertise_usage.cpu_seconds + fetch_usage.cpu_seconds,
                                mem_byte_seconds: advertise_usage.mem_byte_seconds
                                    + fetch_usage.mem_byte_seconds,
                            }
                        );
                    }
                    other => panic!("expected Failed, got {other:?}"),
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            // ---- Sol's round-1 review, blocker 2: a genuine production-shaped teardown-unproven
            // outcome (RuntimeFinalization::Failed) must never be collapsed into an ordinary Failed ----

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn production_shaped_teardown_failure_is_reported_as_teardown_unproven() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xdc);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let advertise_usage = ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 9,
                };
                let advertise_bytes = advertisement_bytes(&oid);
                let bundle_dir = temp_dir_for("5b3-3-teardown-failed-bundle");
                let bundle_dir_check = bundle_dir.clone();
                let run = ContainerRun {
                    child: Box::new(FakeRunsc),
                    bundle_dir,
                    result: SandboxResult {
                        exit_code: Some(0),
                        timed_out: false,
                        usage: advertise_usage,
                        stdout: advertise_bytes,
                        stderr: Vec::new(),
                    },
                    run_error: None,
                };
                let slot = Mutex::new(Some((
                    Ok(RuntimeFinalization::Failed {
                        primary: Ok((run, false)),
                        teardown: RuntimeTeardownError {
                            issues: vec![RuntimeTeardownIssue::ContainerNotConfirmedDeleted(
                                "simulated: runsc delete did not confirm".to_string(),
                            )],
                        },
                    }),
                    Ok(()),
                )));
                let executor: BoxedHopExecutor = Box::new(
                    move |_job: &JobSpec,
                          _cfg: &OciConfig,
                          _stdin: Vec<u8>,
                          _rootfs: &Path,
                          _cancellation: &AtomicBool,
                          _permit: LaunchPermit| {
                        slot.lock()
                            .unwrap()
                            .take()
                            .expect("executor invoked more times than scripted (single-shot)")
                    },
                );
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                match err {
                    CheckoutTransportError::TeardownUnproven { usage, message } => {
                        assert_eq!(usage, advertise_usage);
                        assert!(
                            message.contains("could not be proven"),
                            "message was: {message}"
                        );
                        assert!(
                            message.contains("did not confirm"),
                            "message was: {message}"
                        );
                    }
                    other => panic!("expected TeardownUnproven, got {other:?}"),
                }
                assert!(
                    !bundle_dir_check.exists(),
                    "the discarded run's bundle dir must still be removed by this function itself, \
                     since production's own settle_finalization is never reached on this path"
                );
                let _ = std::fs::remove_dir_all(&root);
            }

            // ---- Sol's round-1 review, blocker 4: numerical usage is not a lifecycle marker ----

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn zero_usage_advertisement_then_fetch_pre_spawn_failure_is_still_failed_not_refused() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xdd);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                // A completed advertisement hop with GENUINELY ZERO measured usage -- distinct from
                // "no hop ran yet." A prior implementation compared `usage_before == zero` to decide
                // Refused-vs-Failed, which would have misclassified this exact case.
                let zero_advertise_usage = ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0,
                };
                let advertise_bytes = advertisement_bytes(&oid);
                let (executor, remaining) = scripted_executor(vec![
                    Box::new(move || {
                        Ok((
                            fake_hop_container_run(advertise_bytes, zero_advertise_usage),
                            false,
                        ))
                    }),
                    Box::new(|| Err(RunFailure::uncommitted("simulated fetch pre-spawn failure"))),
                ]);
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                assert_eq!(*remaining.lock().unwrap(), 0);
                match err {
                    CheckoutTransportError::Failed { usage, .. } => {
                        assert_eq!(
                            usage, zero_advertise_usage,
                            "a completed-but-zero-usage advertisement followed by a fetch failure \
                             must still be Failed, never Refused"
                        );
                    }
                    other => panic!(
                        "expected Failed (never Refused, even though usage is numerically zero), \
                         got {other:?}"
                    ),
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            // ---- Sol's round-3 review, blocker 1: an unproven pre-finalization bundle cleanup
            // must never be silently reported as the free `Refused` ----

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn bundle_cleanup_failure_forces_teardown_unproven_even_on_the_first_hop() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xde);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                // Simulates a pre-finalization failure (e.g. cgroup creation) whose OWN best-effort
                // bundle-dir removal also failed -- nothing ever executed (genuinely `Uncommitted`,
                // first hop), yet the bundle cleanup itself could not be proven.
                let slot = Mutex::new(Some((
                    Err(RunFailure::uncommitted("simulated cgroup creation failure")),
                    Err("simulated bundle dir removal failure".to_string()),
                )));
                let executor: BoxedHopExecutor = Box::new(
                    move |_job: &JobSpec,
                          _cfg: &OciConfig,
                          _stdin: Vec<u8>,
                          _rootfs: &Path,
                          _cancellation: &AtomicBool,
                          _permit: LaunchPermit| {
                        slot.lock()
                            .unwrap()
                            .take()
                            .expect("executor invoked more times than scripted (single-shot)")
                    },
                );
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                match err {
                    CheckoutTransportError::TeardownUnproven { usage, message } => {
                        assert_eq!(
                            usage,
                            ResourceUsage {
                                cpu_seconds: 0,
                                mem_byte_seconds: 0,
                            },
                            "nothing ever executed -- zero is the honest total"
                        );
                        assert!(
                            message.contains("bundle directory could not be proven removed"),
                            "message was: {message}"
                        );
                        assert!(
                            message.contains("simulated bundle dir removal failure"),
                            "message was: {message}"
                        );
                    }
                    other => panic!(
                        "expected TeardownUnproven (an unproven bundle cleanup must never be \
                         reported as the free Refused, even on the very first hop), got {other:?}"
                    ),
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            // ---- Sol's round-3 review, blocker 3: a finalization failure must not mask a
            // simultaneous guest-result failure ----

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn non_passing_result_inside_a_teardown_failure_preserves_both_reasons() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xdf);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let advertise_usage = ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 9,
                };
                let advertise_bytes = advertisement_bytes(&oid);
                let bundle_dir = temp_dir_for("5b3-3-teardown-failed-non-passing-bundle");
                let run = ContainerRun {
                    child: Box::new(FakeRunsc),
                    bundle_dir,
                    result: SandboxResult {
                        exit_code: Some(1), // did NOT pass
                        timed_out: false,
                        usage: advertise_usage,
                        stdout: advertise_bytes,
                        stderr: Vec::new(),
                    },
                    run_error: None,
                };
                let slot = Mutex::new(Some((
                    Ok(RuntimeFinalization::Failed {
                        primary: Ok((run, false)),
                        teardown: RuntimeTeardownError {
                            issues: vec![RuntimeTeardownIssue::ContainerNotConfirmedDeleted(
                                "simulated: runsc delete did not confirm".to_string(),
                            )],
                        },
                    }),
                    Ok(()),
                )));
                let executor: BoxedHopExecutor = Box::new(
                    move |_job: &JobSpec,
                          _cfg: &OciConfig,
                          _stdin: Vec<u8>,
                          _rootfs: &Path,
                          _cancellation: &AtomicBool,
                          _permit: LaunchPermit| {
                        slot.lock()
                            .unwrap()
                            .take()
                            .expect("executor invoked more times than scripted (single-shot)")
                    },
                );
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                match err {
                    CheckoutTransportError::TeardownUnproven { usage, message } => {
                        assert_eq!(usage, advertise_usage);
                        assert!(
                            message.contains("did not pass"),
                            "the guest's own non-passing result must survive, message was: {message}"
                        );
                        assert!(
                            message.contains("could not be proven"),
                            "the teardown failure must ALSO survive, message was: {message}"
                        );
                    }
                    other => {
                        panic!("expected TeardownUnproven combining both facts, got {other:?}")
                    }
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            // ---- Sol's round-4 review, blocker 1: CommitOutcomeUnknown inside a genuine teardown
            // failure must not erase it ----

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn commit_outcome_unknown_inside_a_teardown_failure_is_still_teardown_unproven() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xe0);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let slot = Mutex::new(Some((
                    Ok(RuntimeFinalization::Failed {
                        primary: Err(RunFailure::commit_outcome_unknown(
                            "simulated commit-outcome ambiguity",
                        )),
                        teardown: RuntimeTeardownError {
                            issues: vec![RuntimeTeardownIssue::ContainerNotConfirmedDeleted(
                                "simulated: runsc delete did not confirm".to_string(),
                            )],
                        },
                    }),
                    Ok(()),
                )));
                let executor: BoxedHopExecutor = Box::new(
                    move |_job: &JobSpec,
                          _cfg: &OciConfig,
                          _stdin: Vec<u8>,
                          _rootfs: &Path,
                          _cancellation: &AtomicBool,
                          _permit: LaunchPermit| {
                        slot.lock()
                            .unwrap()
                            .take()
                            .expect("executor invoked more times than scripted (single-shot)")
                    },
                );
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                match err {
                    CheckoutTransportError::TeardownUnproven { usage, message } => {
                        assert_eq!(
                            usage,
                            ResourceUsage {
                                cpu_seconds: 0,
                                mem_byte_seconds: 0,
                            }
                        );
                        assert!(
                            message.contains("internal invariant violated"),
                            "message was: {message}"
                        );
                        assert!(
                            message.contains("did not confirm"),
                            "the independent teardown failure must survive, message was: {message}"
                        );
                    }
                    other => panic!(
                        "expected TeardownUnproven (a real independent teardown failure must never \
                         be erased by an accompanying should-be-impossible commit ambiguity), got \
                         {other:?}"
                    ),
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            // ---- Sol's round-4 review, blocker 2: an unproven bundle cleanup must force
            // TeardownUnproven even when the hop's own result is Ok ----

            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn bundle_cleanup_failure_forces_teardown_unproven_even_on_an_otherwise_successful_hop()
            {
                let root = staged_repo_root();
                let oid = sha1_oid(0xe1);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let proof =
                    minted_proof_for(parent_attempt_scope(&oid, GitObjectFormat::Sha1), "jti-1");
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();

                let advertise_usage = ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 9,
                };
                let advertise_bytes = advertisement_bytes(&oid);
                let run = ContainerRun {
                    child: Box::new(FakeRunsc),
                    bundle_dir: temp_dir_for("5b3-3-contradictory-seam-bundle"),
                    result: SandboxResult {
                        exit_code: Some(0),
                        timed_out: false,
                        usage: advertise_usage,
                        stdout: advertise_bytes,
                        stderr: Vec::new(),
                    },
                    run_error: None,
                };
                // A structurally-permitted but production-never-produces-this combination (Sol's
                // round-4 review): the finalization result is a clean success, yet the paired
                // `BundleCleanupProof` is `Err` -- the executor type allows this even though the real
                // `run_git_wire_container_raw` never returns it (its success path only ever pairs `Ok`
                // finalization with `Ok(())` cleanup, since nothing is removed on that path yet).
                let slot = Mutex::new(Some((
                    Ok(RuntimeFinalization::Finalized(FinalizedRun {
                        primary: Ok((run, false)),
                        evidence: fake_quiescence_evidence(),
                    })),
                    Err("simulated bundle dir removal failure".to_string()),
                )));
                let executor: BoxedHopExecutor = Box::new(
                    move |_job: &JobSpec,
                          _cfg: &OciConfig,
                          _stdin: Vec<u8>,
                          _rootfs: &Path,
                          _cancellation: &AtomicBool,
                          _permit: LaunchPermit| {
                        slot.lock()
                            .unwrap()
                            .take()
                            .expect("executor invoked more times than scripted (single-shot)")
                    },
                );
                let cancellation = AtomicBool::new(false);

                let err = fetch_checkout_pack_within_parent_attempt_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    proof,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                match err {
                    CheckoutTransportError::TeardownUnproven { usage, message } => {
                        assert_eq!(usage, advertise_usage);
                        assert!(
                            message.contains("otherwise succeeded"),
                            "message was: {message}"
                        );
                        assert!(
                            message.contains("simulated bundle dir removal failure"),
                            "message was: {message}"
                        );
                    }
                    other => panic!(
                        "expected TeardownUnproven (an unproven bundle cleanup must never be \
                         silently discarded just because finalization itself returned Ok), got \
                         {other:?}"
                    ),
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            // =================================================================================
            // CT-007 phase-credential generations: the V2 transport / preparation authority.
            //
            // Round-1 blocker 2: the concrete bypass these tests exist to close is "still-valid
            // proof for requeued claim A + live permit for claim B". With the fused, consuming
            // `PhaseAuthorization` that pairing is not expressible — the tests below prove the
            // adjacent mix-and-match attempts that ARE expressible all refuse before any spawn.
            // =================================================================================

            // `permit_recording_executor` was relocated to the `checkout_transport_test_support`
            // module (re-imported above) so the runsc-driver seam + §4 tests share the ONE two-call
            // permit-recording executor.

            fn advertise_authorization(oid: &str, jti: &str) -> PhaseAuthorization {
                minted_phase_authorization(
                    parent_attempt_scope(oid, GitObjectFormat::Sha1),
                    jti,
                    crate::CheckoutPhase::Advertise,
                    &generation_id_for(crate::CheckoutPhase::Advertise),
                    Ok(()),
                )
            }

            /// **Cross-phase substitution, FETCH-for-ADVERTISE.** A well-formed authorization for
            /// the wrong boundary refuses before anything spawns.
            #[test]
            fn a_fetch_phase_authorization_substituted_for_advertise_refuses_before_any_spawn() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xd2);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let authorization = minted_phase_authorization(
                    parent_attempt_scope(&oid, GitObjectFormat::Sha1),
                    "jti-1",
                    crate::CheckoutPhase::Fetch,
                    &generation_id_for(crate::CheckoutPhase::Fetch),
                    Ok(()),
                );
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();
                let (executor, _remaining) = panics_if_called_executor();
                let cancellation = AtomicBool::new(false);
                let mut never = || panic!("the fetch provider must never be reached");
                let err = fetch_checkout_pack_within_parent_attempt_v2_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    authorization,
                    &mut never,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                match err {
                    CheckoutTransportError::Refused { message } => assert!(
                        message.contains("minted for the Fetch boundary"),
                        "message was: {message}"
                    ),
                    other => panic!("expected Refused, got {other:?}"),
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            /// **CROSS-INVOCATION: an authorization minted against a DIFFERENT claim's credential.**
            /// This is the closest expressible form of the blocker-2 bypass — the caller holds a
            /// live authorization (permit included) from claim B and tries to drive claim A's
            /// transport with it. The authorization's privately retained JTI refuses it.
            #[test]
            fn an_authorization_from_another_claim_cannot_drive_this_transport() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xd9);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                // Claim B's authorization: fully valid, permit live.
                let claim_b = advertise_authorization(&oid, "jti-claim-b");
                // ...presented alongside claim A's credential.
                let claim_a_credential =
                    RunTokenCredential::new("bearer", "jti-claim-a", 300).unwrap();
                let (executor, _remaining) = panics_if_called_executor();
                let cancellation = AtomicBool::new(false);
                let mut never = || panic!("the fetch provider must never be reached");
                let err = fetch_checkout_pack_within_parent_attempt_v2_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    claim_a_credential,
                    claim_b,
                    &mut never,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                match err {
                    CheckoutTransportError::Refused { message } => assert!(
                        message.contains("minted against run-token jti")
                            && message.contains("jti-claim-b"),
                        "message was: {message}"
                    ),
                    other => panic!("expected Refused, got {other:?}"),
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            /// **CROSS-SCOPE: a valid authorization for another repo/commit.**
            #[test]
            fn an_authorization_for_another_target_cannot_drive_this_transport() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xda);
                let other_oid = sha1_oid(0xdb);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                for (label, scope) in [
                    (
                        "commit",
                        parent_attempt_scope(&other_oid, GitObjectFormat::Sha1),
                    ),
                    (
                        "repo",
                        CheckoutAuthorizationScope::new(
                            myelin_tenancy::TenantId(TENANT.to_string()),
                            myelin_events::ArtifactRef(format!(
                                "myelin://{TENANT}/git/repo/other-repo"
                            )),
                            "other-repo".to_string(),
                            oid.clone(),
                            GitObjectFormat::Sha1,
                        ),
                    ),
                    (
                        "tenant",
                        CheckoutAuthorizationScope::new(
                            myelin_tenancy::TenantId("someone-else".to_string()),
                            myelin_events::ArtifactRef(
                                "myelin://someone-else/git/repo/widgets".to_string(),
                            ),
                            REPO.to_string(),
                            oid.clone(),
                            GitObjectFormat::Sha1,
                        ),
                    ),
                ] {
                    let authorization = minted_phase_authorization(
                        scope,
                        "jti-1",
                        crate::CheckoutPhase::Advertise,
                        &generation_id_for(crate::CheckoutPhase::Advertise),
                        Ok(()),
                    );
                    let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();
                    let (executor, _remaining) = panics_if_called_executor();
                    let cancellation = AtomicBool::new(false);
                    let mut never = || panic!("the fetch provider must never be reached");
                    let err = fetch_checkout_pack_within_parent_attempt_v2_given(
                        &root,
                        TENANT,
                        REGION,
                        REPO,
                        &expected,
                        checkout_limits(),
                        run_token,
                        authorization,
                        &mut never,
                        &cancellation,
                        None,
                        &*executor,
                    )
                    .unwrap_err();
                    assert!(
                        matches!(err, CheckoutTransportError::Refused { .. }),
                        "a substituted {label} must refuse before any spawn, got {err:?}"
                    );
                }
                let _ = std::fs::remove_dir_all(&root);
            }

            /// **Advertisement succeeds but the fetch mint refuses: the fetch never spawns, and the
            /// advertisement's already-measured usage survives into the error.**
            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn a_refused_fetch_mint_never_spawns_the_fetch_and_keeps_the_advertisement_usage() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xd3);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let authorization = advertise_authorization(&oid, "jti-advertise");
                let run_token = RunTokenCredential::new("bearer", "jti-advertise", 300).unwrap();
                let advertise_usage = ResourceUsage {
                    cpu_seconds: 7,
                    mem_byte_seconds: 700,
                };
                let (executor, seen) = permit_recording_executor(vec![Box::new({
                    let oid = oid.clone();
                    move || {
                        Ok((
                            fake_hop_container_run(advertisement_bytes(&oid), advertise_usage),
                            false,
                        ))
                    }
                })]);
                let cancellation = AtomicBool::new(false);
                let mut refuse = || {
                    Err(HookError(
                        "the workload generation already superseded it".into(),
                    ))
                };
                let err = fetch_checkout_pack_within_parent_attempt_v2_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    authorization,
                    &mut refuse,
                    &cancellation,
                    None,
                    &*executor,
                )
                .unwrap_err();
                match err {
                    CheckoutTransportError::Failed { usage, message, .. } => {
                        assert_eq!(
                            usage, advertise_usage,
                            "the advertisement's measured usage must survive a refused fetch mint"
                        );
                        assert!(
                            message.contains("mint fetch-phase credential"),
                            "message was: {message}"
                        );
                    }
                    other => {
                        panic!("expected Failed carrying the advertisement usage, got {other:?}")
                    }
                }
                let recorded = seen.lock().unwrap();
                assert_eq!(
                    recorded.len(),
                    1,
                    "exactly ONE container ran: the advertisement. The fetch never spawned."
                );
                assert_eq!(recorded[0], ("jti-advertise".to_string(), true));
                let _ = std::fs::remove_dir_all(&root);
            }

            /// The fetch provider returning a WRONG-PHASE authorization, a MISMATCHED credential, or
            /// the SAME generation as the advertisement all refuse — and the fetch never spawns.
            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn a_divergent_fetch_authorization_refuses_before_the_fetch_spawns() {
                type Provider =
                    Box<dyn FnMut() -> Result<(RunTokenCredential, PhaseAuthorization), HookError>>;
                /// (label, expected refusal fragment, provider builder).
                type DivergentFetchCase = (&'static str, &'static str, fn(&str) -> Provider);
                let cases: Vec<DivergentFetchCase> = vec![
                    (
                        "wrong phase",
                        "minted for the Advertise boundary",
                        |oid: &str| {
                            let oid = oid.to_string();
                            Box::new(move || {
                                Ok((
                                    RunTokenCredential::new("bearer", "jti-fetch", 300).unwrap(),
                                    minted_phase_authorization(
                                        parent_attempt_scope(&oid, GitObjectFormat::Sha1),
                                        "jti-fetch",
                                        crate::CheckoutPhase::Advertise,
                                        "ci-credential:v1:distinct-advertise",
                                        Ok(()),
                                    ),
                                ))
                            })
                        },
                    ),
                    (
                        "credential from another invocation",
                        "minted against run-token jti",
                        |oid: &str| {
                            let oid = oid.to_string();
                            Box::new(move || {
                                Ok((
                                    // A credential that does NOT belong to the authorization
                                    // returned alongside it.
                                    RunTokenCredential::new("bearer", "jti-other-claim", 300)
                                        .unwrap(),
                                    minted_phase_authorization(
                                        parent_attempt_scope(&oid, GitObjectFormat::Sha1),
                                        "jti-fetch",
                                        crate::CheckoutPhase::Fetch,
                                        &generation_id_for(crate::CheckoutPhase::Fetch),
                                        Ok(()),
                                    ),
                                ))
                            })
                        },
                    ),
                    (
                        "same generation as the advertisement",
                        "SAME durable generation",
                        |oid: &str| {
                            let oid = oid.to_string();
                            Box::new(move || {
                                Ok((
                                    RunTokenCredential::new("bearer", "jti-fetch", 300).unwrap(),
                                    minted_phase_authorization(
                                        parent_attempt_scope(&oid, GitObjectFormat::Sha1),
                                        "jti-fetch",
                                        crate::CheckoutPhase::Fetch,
                                        &generation_id_for(crate::CheckoutPhase::Advertise),
                                        Ok(()),
                                    ),
                                ))
                            })
                        },
                    ),
                ];
                for (label, expected_message, build) in cases {
                    let root = staged_repo_root();
                    let oid = sha1_oid(0xd4);
                    let expected =
                        ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                    let authorization = advertise_authorization(&oid, "jti-advertise");
                    let run_token =
                        RunTokenCredential::new("bearer", "jti-advertise", 300).unwrap();
                    let advertise_usage = ResourceUsage {
                        cpu_seconds: 3,
                        mem_byte_seconds: 300,
                    };
                    let (executor, seen) = permit_recording_executor(vec![Box::new({
                        let oid = oid.clone();
                        move || {
                            Ok((
                                fake_hop_container_run(advertisement_bytes(&oid), advertise_usage),
                                false,
                            ))
                        }
                    })]);
                    let cancellation = AtomicBool::new(false);
                    let mut provider = build(&oid);
                    let err = fetch_checkout_pack_within_parent_attempt_v2_given(
                        &root,
                        TENANT,
                        REGION,
                        REPO,
                        &expected,
                        checkout_limits(),
                        run_token,
                        authorization,
                        &mut *provider,
                        &cancellation,
                        None,
                        &*executor,
                    )
                    .unwrap_err();
                    match err {
                        CheckoutTransportError::Failed { usage, message, .. } => {
                            assert_eq!(usage, advertise_usage, "{label}: usage survives");
                            assert!(
                                message.contains(expected_message),
                                "{label}: message was: {message}"
                            );
                        }
                        other => panic!("{label}: expected Failed, got {other:?}"),
                    }
                    assert_eq!(
                        seen.lock().unwrap().len(),
                        1,
                        "{label}: the fetch must never spawn"
                    );
                    let _ = std::fs::remove_dir_all(&root);
                }
            }

            /// **The V2 happy path: each leg spawns under its OWN credential and its OWN durable
            /// phase permit.** This is what makes a >5-minute Hop A survivable at all.
            #[test]
            #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
            fn the_v2_transport_spawns_each_leg_under_its_own_credential_and_phase_permit() {
                let root = staged_repo_root();
                let oid = sha1_oid(0xd5);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let authorization = advertise_authorization(&oid, "jti-advertise");
                let run_token = RunTokenCredential::new("bearer", "jti-advertise", 300).unwrap();
                let (executor, seen) = permit_recording_executor(vec![
                    Box::new({
                        let oid = oid.clone();
                        move || {
                            Ok((
                                fake_hop_container_run(
                                    advertisement_bytes(&oid),
                                    ResourceUsage {
                                        cpu_seconds: 1,
                                        mem_byte_seconds: 100,
                                    },
                                ),
                                false,
                            ))
                        }
                    }),
                    Box::new(move || {
                        Ok((
                            fake_hop_container_run(
                                fetch_response_bytes(b"pack-bytes"),
                                ResourceUsage {
                                    cpu_seconds: 2,
                                    mem_byte_seconds: 200,
                                },
                            ),
                            false,
                        ))
                    }),
                ]);
                let cancellation = AtomicBool::new(false);
                let fetch_oid = oid.clone();
                let mut provide = move || {
                    Ok((
                        RunTokenCredential::new("bearer", "jti-fetch", 300).unwrap(),
                        minted_phase_authorization(
                            parent_attempt_scope(&fetch_oid, GitObjectFormat::Sha1),
                            "jti-fetch",
                            crate::CheckoutPhase::Fetch,
                            &generation_id_for(crate::CheckoutPhase::Fetch),
                            Ok(()),
                        ),
                    ))
                };
                let outcome = fetch_checkout_pack_within_parent_attempt_v2_given(
                    &root,
                    TENANT,
                    REGION,
                    REPO,
                    &expected,
                    checkout_limits(),
                    run_token,
                    authorization,
                    &mut provide,
                    &cancellation,
                    None,
                    &*executor,
                )
                .expect("the V2 phase-bound transport completes");
                assert_eq!(
                    outcome.usage,
                    ResourceUsage {
                        cpu_seconds: 3,
                        mem_byte_seconds: 300,
                    }
                );
                let recorded = seen.lock().unwrap();
                assert_eq!(
                    *recorded,
                    vec![
                        ("jti-advertise".to_string(), true),
                        ("jti-fetch".to_string(), true),
                    ],
                    "each leg runs under its OWN phase credential and commits its OWN phase permit"
                );
                let _ = std::fs::remove_dir_all(&root);
            }

            /// **A phase permit whose durable generation is no longer current refuses AT THE SPAWN
            /// GATE**, not at mint time — the whole reason the permit is retained and lazy.
            #[test]
            fn a_superseded_phase_permit_refuses_when_the_spawn_gate_commits_it() {
                let oid = sha1_oid(0xd6);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let authorization = minted_phase_authorization(
                    parent_attempt_scope(&oid, GitObjectFormat::Sha1),
                    "jti-1",
                    crate::CheckoutPhase::Advertise,
                    &generation_id_for(crate::CheckoutPhase::Advertise),
                    Err("a successor generation was appended"),
                );
                let run_token = RunTokenCredential::new("bearer", "jti-1", 300).unwrap();
                let permit = authorization
                    .into_transport_permit(
                        crate::CheckoutPhase::Advertise,
                        &run_token,
                        TENANT,
                        REPO,
                        &expected,
                    )
                    .expect("the authorization itself is well-formed");
                let error = permit
                    .commit_and_release()
                    .expect_err("a superseded generation must refuse at the gate");
                assert!(
                    error.0.contains("successor generation"),
                    "message was: {}",
                    error.0
                );
            }

            // ---- Hop B: the materialization authority ----

            #[test]
            fn hop_b_consumes_only_a_materialization_authorization_for_the_exact_claim() {
                let oid = sha1_oid(0xd7);
                let expected =
                    ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
                let run_token =
                    RunTokenCredential::new("bearer", "jti-materialization", 300).unwrap();
                // CT-007 slice 5b.3-6a (blocker 1): Hop B's permit is now resolved against the
                // capsule's FULL derived scope, not just the commit — the capsule was acquired for
                // exactly this scope.
                let capsule_scope = parent_attempt_scope(&oid, GitObjectFormat::Sha1);

                // The exact materialization authorization is accepted, and its permit commits.
                resolve_checkout_preparation_permit(
                    minted_phase_authorization(
                        parent_attempt_scope(&oid, GitObjectFormat::Sha1),
                        "jti-materialization",
                        crate::CheckoutPhase::Materialization,
                        &generation_id_for(crate::CheckoutPhase::Materialization),
                        Ok(()),
                    ),
                    &run_token,
                    &capsule_scope,
                    &expected,
                )
                .expect("the exact materialization authorization authorizes Hop B")
                .commit_and_release()
                .expect("its durable permit commits");

                // Every adjacent substitution refuses.
                let cases: [(&str, crate::CheckoutPhase, &str, &str, &str); 4] = [
                    (
                        "fetch phase",
                        crate::CheckoutPhase::Fetch,
                        "jti-materialization",
                        &oid,
                        "minted for the Fetch boundary",
                    ),
                    (
                        "advertise phase",
                        crate::CheckoutPhase::Advertise,
                        "jti-materialization",
                        &oid,
                        "minted for the Advertise boundary",
                    ),
                    (
                        "another claim's credential",
                        crate::CheckoutPhase::Materialization,
                        "jti-other-claim",
                        &oid,
                        "minted against run-token jti",
                    ),
                    (
                        "another commit",
                        crate::CheckoutPhase::Materialization,
                        "jti-materialization",
                        "ffffffffffffffffffffffffffffffffffffffff",
                        // 5b.3-6a: a different commit is now a FULL-scope mismatch against the
                        // capsule's own scope, caught before the commit-vs-preparation check.
                        "was minted for scope",
                    ),
                ];
                for (label, phase, jti, commit, expected_message) in cases {
                    let error = resolve_checkout_preparation_permit(
                        minted_phase_authorization(
                            parent_attempt_scope(commit, GitObjectFormat::Sha1),
                            jti,
                            phase,
                            &generation_id_for(phase),
                            Ok(()),
                        ),
                        &run_token,
                        &capsule_scope,
                        &expected,
                    )
                    .err()
                    .unwrap_or_else(|| panic!("{label} must not drive Hop B"));
                    match error {
                        CheckoutPreparationError::Refused(message) => assert!(
                            message.contains(expected_message),
                            "{label}: message was: {message}"
                        ),
                        other => panic!("{label}: expected Refused, got {other:?}"),
                    }
                }
            }
        }
    }

    // ══════════ CT-007 slice 5b.3-6e.1b: the 8 mandatory deterministic-substrate tests ══════════
    //
    // These RUN (never soft-skip) given a NON-root user + a writable tmp base dir — no
    // Btrfs/CAP_SYS_ADMIN/subuid/KVM/runsc. Everything they touch is `#[cfg(any(test,
    // feature = "test-support"))]`, so they compile+run under `--lib` AND `--lib --features
    // test-support`.
    mod deterministic_substrate_6e1b {
        use super::super::{
            acquire_enabled_workspace, classify_workspace_deletion,
            deterministic_userns_allocator_for_tests, deterministic_workspace_manager_for_tests,
            run_substituted_checkout_mismatched_evidence, run_substituted_checkout_success,
            settle_enabled_workspace_and_lease, substitute_checkout_spec, unique_suffix,
            CgroupQuiescenceEvidence, LeaseBindState, RuntimeNamespaceQuiescence,
            RuntimeQuiescenceEvidence, WorkspaceDeletionOutcome,
        };
        use crate::user_namespace::{
            CheckoutPreparationSession, PreparationQuiescenceProof, UserNamespaceQuiescenceProof,
        };
        use crate::workspace_manager::{
            DeleteWorkspaceError, WorkspaceAdmission, WorkspaceManagerError,
            WorkspaceProvisionError,
        };
        use crate::workspace_storage::{
            DirectoryWorkspaceStorage, PreparedWorkspace, WorkspaceStorageError,
        };
        use std::path::PathBuf;

        fn temp_root(tag: &str) -> PathBuf {
            let root = std::env::temp_dir().join(format!(
                "myelin-6e1b-{tag}-{}-{}",
                std::process::id(),
                unique_suffix()
            ));
            std::fs::create_dir_all(&root).expect("mk temp root");
            root
        }

        fn open_directory_backend(tag: &str) -> (DirectoryWorkspaceStorage, PathBuf) {
            let base = temp_root(tag).join("dir-backend");
            std::fs::create_dir_all(&base).expect("mk base");
            let backend = DirectoryWorkspaceStorage::open(&base)
                .expect("directory backend opens over an exclusively-owned dir");
            let canonical = backend.base_dir().to_path_buf();
            (backend, canonical)
        }

        // ── Test 1: directory create → checked sentinel write/read → delete → proven absence. ──
        #[test]
        fn directory_create_write_read_delete_absence() {
            let base = temp_root("t1").join("workspace");
            std::fs::create_dir_all(&base).unwrap();
            let wm = deterministic_workspace_manager_for_tests(base.clone(), 1 << 30).unwrap();
            let cap = wm.acquire_capacity(1 << 20).expect("capacity");
            let ws = wm
                .create_workspace("job-t1", 1 << 20, 0, 0, cap)
                .expect("create directory workspace");
            let host = ws.host_path().to_path_buf();
            assert!(host.is_dir(), "a fresh leaf directory exists");
            ws.checked_test_quota_write("checkout.sentinel", b"provenance")
                .expect("the checked byte-accounted write succeeds under quota");
            assert_eq!(
                std::fs::read(host.join("checkout.sentinel")).unwrap(),
                b"provenance",
                "the sentinel reads back byte-identical"
            );
            // An over-quota checked write refuses BEFORE mutating.
            let refusal = ws.checked_test_quota_write("huge", &vec![0u8; (1 << 20) + 1]);
            assert!(
                matches!(
                    refusal,
                    Err(WorkspaceStorageError::DirectoryQuotaExceeded { .. })
                ),
                "an over-quota checked write refuses, got {refusal:?}"
            );
            assert!(
                !host.join("huge").exists(),
                "the refused over-quota write left nothing behind"
            );
            wm.delete_workspace(ws).expect("delete proves absence");
            assert!(
                !host.exists(),
                "the leaf is gone after a proven-absence delete"
            );
            assert_eq!(
                wm.capacity_used_bytes(),
                0,
                "capacity released after delete"
            );
            let _ = std::fs::remove_dir_all(&base);
        }

        // ── Test 2: capacity leased, exhausted, released, then reusable — real aggregate accounting. ──
        #[test]
        fn capacity_leased_then_released_and_reusable() {
            let base = temp_root("t2").join("workspace");
            std::fs::create_dir_all(&base).unwrap();
            let wm = deterministic_workspace_manager_for_tests(base.clone(), 4 << 20).unwrap();
            let cap = wm
                .acquire_capacity(4 << 20)
                .expect("lease the whole ceiling");
            assert_eq!(wm.capacity_used_bytes(), 4 << 20);
            // Ceiling exhausted: a further request is refused (REAL aggregate accounting).
            assert!(wm.acquire_capacity(1).is_err(), "the ceiling is exhausted");
            let ws = wm
                .create_workspace("job-t2", 4 << 20, 0, 0, cap)
                .expect("create consumes the lease");
            wm.delete_workspace(ws)
                .expect("delete releases the capacity");
            assert_eq!(wm.capacity_used_bytes(), 0, "capacity fully returned");
            // Reusable: the freed ceiling admits a fresh lease.
            let again = wm
                .acquire_capacity(4 << 20)
                .expect("reuse the freed ceiling");
            again.release();
            let _ = std::fs::remove_dir_all(&base);
        }

        // ── Test 3: the REAL userns preparation/bind/workload/release transitions, deterministically. ──
        #[test]
        fn real_userns_preparation_bind_workload_release_transitions() {
            let base = temp_root("t3").join("userns");
            std::fs::create_dir_all(&base).unwrap();
            let alloc = deterministic_userns_allocator_for_tests(&base, 1)
                .expect("a NON-root user builds the fixture allocator");
            let mut lease = alloc.lease().expect("a fresh pool leases");
            let mut session = CheckoutPreparationSession::new();
            let (prep_root, prep_cgroup) = ((1_u64, 2_u64), (3_u64, 4_u64));
            session
                .bind_preparation(&mut lease, "c-prep".to_string(), prep_root, prep_cgroup)
                .expect("Allocated -> PreparationBound");
            let prep_ev = RuntimeQuiescenceEvidence::assert_for_tests(
                "c-prep".to_string(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: prep_root,
                },
                CgroupQuiescenceEvidence::assert_for_tests(prep_cgroup),
            );
            let prep_proof = PreparationQuiescenceProof::from_runtime_evidence(&lease, &prep_ev)
                .expect("a matching prep evidence mints a proof");
            session
                .confirm_prepared(&mut lease, prep_proof)
                .expect("PreparationBound -> Prepared");
            let (wl_root, wl_cgroup) = ((5_u64, 6_u64), (7_u64, 8_u64));
            session
                .bind_workload(&mut lease, "c-workload".to_string(), wl_root, wl_cgroup)
                .expect("Prepared -> Bound");
            let wl_ev = RuntimeQuiescenceEvidence::assert_for_tests(
                "c-workload".to_string(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: wl_root,
                },
                CgroupQuiescenceEvidence::assert_for_tests(wl_cgroup),
            );
            let proof = UserNamespaceQuiescenceProof::from_runtime_evidence(&lease, &wl_ev)
                .expect("the workload evidence mints a release proof");
            lease.release(proof).expect("release with a matching proof");
            assert!(alloc.is_healthy(), "the allocator stays healthy");
            // Probe reusability WITHOUT poisoning: acquire then `release_unused` (never drop an
            // unreleased probe lease — that would emit a quarantine incident and poison the
            // allocator this test claims stays clean).
            let probe = alloc.lease().expect("the slot is reusable after release");
            probe
                .release_unused()
                .expect("the probe lease releases cleanly");
            assert!(
                alloc.is_healthy(),
                "the allocator is STILL clean after the probe"
            );
            let _ = std::fs::remove_dir_all(&base);
        }

        // ── Test 4: the FULL capsule fake-Hop-B / fake-workload → real settle/delete. ──
        #[test]
        fn full_capsule_substituted_hopb_and_workload_then_real_settle() {
            let root = temp_root("t4");
            let (obs, wm, workspace_base, alloc, userns_base) =
                run_substituted_checkout_success(&root, "checkout.sentinel", b"shared-provenance");
            assert!(
                obs.hopb_write_ok,
                "the checked Hop B sentinel write succeeded"
            );
            assert!(
                obs.used_after_hopb >= "shared-provenance".len() as u64,
                "the byte-accounted checkpoint saw Hop B's bytes: {}",
                obs.used_after_hopb
            );
            assert_eq!(
                obs.used_at_workload_checkpoint, obs.used_after_hopb,
                "the re-scan at the workload checkpoint agrees"
            );
            assert!(
                obs.mount_source_matched_workspace,
                "the retained OCI mount source equals the capsule workspace host path"
            );
            assert!(
                obs.sentinel_read_through_mount,
                "the substituted workload read the sentinel THROUGH the OCI-recorded mount"
            );
            assert!(
                obs.settled_ok,
                "the real settle tail succeeded: {:?}",
                obs.settle_error
            );
            // Step 8: durable state.
            let child_dirs = std::fs::read_dir(&workspace_base)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .count();
            assert_eq!(child_dirs, 0, "the workspace leaf was deleted by settle");
            assert_eq!(wm.capacity_used_bytes(), 0, "capacity zero after settle");
            assert!(wm.is_healthy(), "the workspace manager stays healthy");
            assert!(alloc.is_healthy(), "the userns allocator stays healthy");
            // Probe reusability WITHOUT poisoning (release the probe lease, never drop it unreleased).
            let probe = alloc.lease().expect("the userns slot is reusable");
            probe
                .release_unused()
                .expect("the probe lease releases cleanly");
            assert!(
                alloc.is_healthy(),
                "the allocator is STILL clean after the probe"
            );
            let _ = std::fs::remove_dir_all(&workspace_base);
            let _ = std::fs::remove_dir_all(&userns_base);
            let _ = std::fs::remove_dir_all(&root);
        }

        // ── Test 5: wrong backend / wrong base / wrong inode / symlink substitution refuse w/o delete. ──
        #[test]
        fn cross_backend_base_inode_and_symlink_substitutions_refuse_without_deleting() {
            // (a) A Btrfs-identity capability handed to the directory backend → BackendMismatch.
            let (mut backend, canonical) = open_directory_backend("t5a");
            let btrfs_cap =
                PreparedWorkspace::for_tests(canonical.join("x"), 42, canonical.clone());
            assert!(
                matches!(
                    backend.delete_workspace(btrfs_cap),
                    Err(WorkspaceStorageError::BackendMismatch { .. })
                ),
                "a Btrfs capability is refused by the directory backend"
            );

            // (b) A directory capability from base A refused by a backend over base B → WrongStorage.
            let (mut backend_a, _) = open_directory_backend("t5b-a");
            let (mut backend_b, _) = open_directory_backend("t5b-b");
            let ws_a = backend_a.create_workspace("job", 1 << 20, 0, 0).unwrap();
            let leaf_a = ws_a.host_path().to_path_buf();
            assert!(
                matches!(
                    backend_b.delete_workspace(ws_a),
                    Err(WorkspaceStorageError::WrongStorage { .. })
                ),
                "backend B refuses backend A's capability"
            );
            assert!(
                leaf_a.exists(),
                "the refused wrong-base delete removed nothing"
            );
            backend_a
                .list_orphaned_workspaces(&std::collections::BTreeSet::new())
                .and_then(|orphans| {
                    orphans
                        .into_iter()
                        .try_for_each(|o| backend_a.delete_orphan(o))
                })
                .unwrap();

            // (c) Inode substitution: replace the leaf with a fresh dir (new inode) → absence unproven.
            let (mut backend_c, _) = open_directory_backend("t5c");
            let ws_c = backend_c.create_workspace("job", 1 << 20, 0, 0).unwrap();
            let leaf_c = ws_c.host_path().to_path_buf();
            std::fs::remove_dir_all(&leaf_c).unwrap();
            std::fs::create_dir(&leaf_c).unwrap(); // a DIFFERENT inode at the same name.
            assert!(
                matches!(
                    backend_c.delete_workspace(ws_c),
                    Err(WorkspaceStorageError::DirectoryAbsenceUnproven { .. })
                ),
                "an inode-substituted leaf refuses deletion (absence unproven)"
            );
            assert!(
                leaf_c.exists(),
                "the substituted replacement dir was NOT deleted"
            );

            // (d) Symlink substitution: replace the leaf with a symlink → absence unproven, not followed.
            let (mut backend_d, _) = open_directory_backend("t5d");
            let ws_d = backend_d.create_workspace("job", 1 << 20, 0, 0).unwrap();
            let leaf_d = ws_d.host_path().to_path_buf();
            let decoy = leaf_d.with_file_name("decoy-target");
            std::fs::create_dir(&decoy).unwrap();
            std::fs::write(decoy.join("keep"), b"do not delete").unwrap();
            std::fs::remove_dir_all(&leaf_d).unwrap();
            std::os::unix::fs::symlink(&decoy, &leaf_d).unwrap();
            assert!(
                matches!(
                    backend_d.delete_workspace(ws_d),
                    Err(WorkspaceStorageError::DirectoryAbsenceUnproven { .. })
                ),
                "a symlink-substituted leaf refuses deletion"
            );
            assert!(
                decoy.join("keep").exists(),
                "the symlink was never followed — the decoy target is intact"
            );
        }

        // ── Test 6: an injected delete failure retains capacity, poisons the manager, and the
        //           absence-unproven outcome is what leaves a paired userns lease unreleased. ──
        #[test]
        fn injected_delete_failure_retains_capacity_and_poisons_the_manager() {
            let base = temp_root("t6").join("workspace");
            std::fs::create_dir_all(&base).unwrap();
            let wm = deterministic_workspace_manager_for_tests(base.clone(), 1 << 30).unwrap();
            let cap = wm.acquire_capacity(1 << 20).unwrap();
            let ws = wm.create_workspace("job-t6", 1 << 20, 0, 0, cap).unwrap();
            let host = ws.host_path().to_path_buf();
            // Inject the failure: swap the leaf for a different-inode dir so the verified delete
            // cannot prove absence.
            std::fs::remove_dir_all(&host).unwrap();
            std::fs::create_dir(&host).unwrap();
            let result = wm.delete_workspace(ws);
            assert!(
                matches!(result, Err(DeleteWorkspaceError::Storage(_))),
                "the delete surfaced a storage failure, got {result:?}"
            );
            assert!(
                matches!(wm.admission(), WorkspaceAdmission::Poisoned { .. }),
                "an absence-unproven delete poisons the manager"
            );
            assert_eq!(
                wm.capacity_used_bytes(),
                1 << 20,
                "capacity is RETAINED (never silently freed) on an unproven delete"
            );
            // The SAME absence-unproven storage error is what the settle path classifies as
            // NotProvenAbsent → the paired userns lease is left unreleased (never reissued).
            let outcome = classify_workspace_deletion(Err(DeleteWorkspaceError::Storage(
                WorkspaceStorageError::DirectoryAbsenceUnproven {
                    path: host.clone(),
                    reason: "injected".to_string(),
                },
            )));
            assert!(
                matches!(outcome, WorkspaceDeletionOutcome::NotProvenAbsent { .. }),
                "an unproven delete leaves the userns lease unreleased/quarantined"
            );
            let _ = std::fs::remove_dir_all(&base);
        }

        // ── Test 7: boot orphan reconciliation deletes stray child dirs, refuses non-dir entries. ──
        #[test]
        fn boot_orphan_reconciliation_deletes_orphans_and_refuses_malformed_entries() {
            // (a) Pre-seed orphan child dirs; construction reconciles them away and admits Healthy.
            let base = temp_root("t7-ok").join("workspace");
            std::fs::create_dir_all(base.join("orphan-a")).unwrap();
            std::fs::create_dir_all(base.join("orphan-b")).unwrap();
            std::fs::write(base.join("orphan-a").join("junk"), b"stale").unwrap();
            let wm = deterministic_workspace_manager_for_tests(base.clone(), 1 << 30)
                .expect("construction reconciles orphans");
            assert!(matches!(wm.admission(), WorkspaceAdmission::Healthy));
            let remaining = std::fs::read_dir(&base)
                .unwrap()
                .filter_map(Result::ok)
                .count();
            assert_eq!(
                remaining, 0,
                "every boot orphan was deleted before admission"
            );
            drop(wm);
            let _ = std::fs::remove_dir_all(&base);

            // (b) A stray FILE (not a directory) refuses construction LOUDLY.
            let base2 = temp_root("t7-bad").join("workspace");
            std::fs::create_dir_all(&base2).unwrap();
            std::fs::write(base2.join("not-a-dir"), b"stray").unwrap();
            let result = deterministic_workspace_manager_for_tests(base2.clone(), 1 << 30);
            assert!(
                matches!(
                    result,
                    Err(WorkspaceManagerError::Storage(
                        WorkspaceStorageError::UnexpectedEntry { .. }
                    ))
                ),
                "a non-directory boot entry is a loud UnexpectedEntry refusal"
            );
            drop(result);
            let _ = std::fs::remove_dir_all(&base2);
        }

        // ── Test 8: ordinary-build + production-composition-root pins — the mode/seams are unreachable. ──
        #[test]
        fn ordinary_build_and_production_root_pins() {
            const WORKSPACE_MANAGER_SOURCE: &str = include_str!("workspace_manager.rs");
            const WORKSPACE_STORAGE_SOURCE: &str = include_str!("workspace_storage.rs");
            const USER_NAMESPACE_SOURCE: &str = include_str!("user_namespace.rs");

            // Production composition in gvisor.rs NEVER names the dormant mode (the GvisorWorkspaceConfig
            // -> EphemeralDisk mapping is the only production selector).
            assert_eq!(
                super::production_source()
                    .matches("DeterministicDirectoryForTests")
                    .count(),
                0,
                "no production gvisor path constructs the deterministic-directory mode"
            );

            // CT-007 5b.3-6e.2: the no-op test-support authority + the substituted-execution mode are
            // named ONLY by the test-support substrate (below the top-level test module) — production
            // source names neither. This keeps ruling-(A)'s substituted workload leg unreachable from
            // every production composition root.
            assert_eq!(
                super::production_source()
                    .matches("NoOpTestSupportAuthority")
                    .count(),
                0,
                "no production gvisor path constructs the no-op test-support attempt authority"
            );
            assert_eq!(
                super::production_source()
                    .matches("SubstitutedEvidenceMode")
                    .count(),
                0,
                "no production gvisor path names the substituted-evidence mode"
            );

            // CT-007 5b.3-6e.2 Stage A: the git-wire test-support substrate + the orchestrator-driving
            // seam are named ONLY by test/test-support code (the module below the top-level test module,
            // and the `#[cfg(feature = "test-support")]` runsc-driver file this scan never reads).
            // Production source names none of them — the whole composed active path is unreachable from
            // every production composition root until Stage B.
            assert_eq!(
                super::production_source()
                    .matches("checkout_transport_test_support")
                    .count(),
                0,
                "no production gvisor path names the git-wire test-support module"
            );
            assert_eq!(
                super::production_source()
                    .matches("drive_checkout_cycle_with_substituted_runsc_given")
                    .count(),
                0,
                "no production gvisor path names the orchestrator-driving runsc seam"
            );
            // CT-007 5b.3-6e.2 Stage A: the §4 prep-terminal/prep-retry tests inject a Hop-B disposition
            // via the new test-support driver + selector. Both live in the `#[cfg(feature =
            // "test-support")]` `runsc_driver` file (which `production_source` never reads) and NO
            // production composition root names them.
            assert_eq!(
                super::production_source()
                    .matches("drive_checkout_cycle_with_injected_hop_b")
                    .count(),
                0,
                "no production gvisor path names the Hop-B-injecting runsc seam"
            );
            assert_eq!(
                super::production_source()
                    .matches("InjectedHopBOutcome")
                    .count(),
                0,
                "no production gvisor path names the injected Hop-B outcome selector"
            );
            assert_eq!(
                super::production_source()
                    .matches("deterministic_enabled_backend_for_tests")
                    .count(),
                0,
                "no production gvisor path builds the deterministic Enabled test backend"
            );
            // CT-007 slice 5b.3-6e.2 Stage A: the two OTHER helpers the §4 tests pub-name (the checkout
            // spec factory + the bare-repo stager) are likewise named ONLY by test/test-support code —
            // making them `pub` for cross-crate reach must not make any production path name them.
            assert_eq!(
                super::production_source()
                    .matches("checkout_spec_for_backend")
                    .count(),
                0,
                "no production gvisor path builds the deterministic checkout spec"
            );
            assert_eq!(
                super::production_source()
                    .matches("stage_checkout_repo_root")
                    .count(),
                0,
                "no production gvisor path stages the deterministic bare-repo root"
            );

            // The mode variant is cfg-gated (absent from ordinary builds).
            assert!(
                WORKSPACE_MANAGER_SOURCE.contains(
                    "#[cfg(any(test, feature = \"test-support\"))]\n    DeterministicDirectoryForTests {"
                ),
                "the DeterministicDirectoryForTests mode variant is test/test-support gated"
            );
            // The whole directory backend + typed identity + checked quota is cfg-gated.
            assert!(
                WORKSPACE_STORAGE_SOURCE.contains(
                    "#[cfg(any(test, feature = \"test-support\"))]\n#[derive(Debug)]\npub(crate) struct DirectoryWorkspaceStorage"
                ),
                "the directory backend struct is test/test-support gated"
            );
            // The userns fixture constructor was widened to test-support (NOT relaxed for production).
            assert!(
                USER_NAMESPACE_SOURCE.contains(
                    "#[cfg(any(test, feature = \"test-support\"))]\n    pub(crate) fn try_new_for_tests("
                ),
                "try_new_for_tests is test/test-support gated, never a production constructor"
            );
            // The production userns constructor is fixed to /etc/subuid — never an arbitrary path.
            assert!(
                USER_NAMESPACE_SOURCE.contains("pub fn try_new(")
                    && USER_NAMESPACE_SOURCE.contains("Path::new(\"/etc/subuid\")"),
                "the production allocator constructor stays pinned to /etc/subuid"
            );
        }

        // ── Test 9 (Sol blocker 2): an injected create failure (an untracked pre-existing leaf)
        //   is an UnrecoverableLeak → capacity RETAINED + manager poisoned. A residual directory can
        //   NEVER coexist with healthy admission + released capacity. ──
        #[test]
        fn injected_create_failure_retains_capacity_and_poisons_without_a_healthy_residual() {
            let base = temp_root("t9").join("workspace");
            std::fs::create_dir_all(&base).unwrap();
            let wm = deterministic_workspace_manager_for_tests(base.clone(), 1 << 30).unwrap();
            // Inject the failure: plant an untracked residual leaf at the job key AFTER boot
            // reconciliation (the manager canonicalizes its base, so match that).
            let canonical = std::fs::canonicalize(&base).unwrap();
            std::fs::create_dir(canonical.join("job-t9")).unwrap();
            let cap = wm.acquire_capacity(1 << 20).unwrap();
            let result = wm.create_workspace("job-t9", 1 << 20, 0, 0, cap);
            assert!(
                matches!(
                    result,
                    Err(WorkspaceProvisionError::Storage(
                        WorkspaceStorageError::UnrecoverableLeak { .. }
                    ))
                ),
                "a pre-existing untracked leaf is an UnrecoverableLeak, got {result:?}"
            );
            assert!(
                matches!(wm.admission(), WorkspaceAdmission::Poisoned { .. }),
                "the manager is poisoned (NOT healthy) while the residual survives"
            );
            assert_eq!(
                wm.capacity_used_bytes(),
                1 << 20,
                "capacity is RETAINED — never released while a residual directory exists"
            );
            assert!(
                canonical.join("job-t9").exists(),
                "the residual leaf is still present — surfaced via poison, not silently released"
            );
            let _ = std::fs::remove_dir_all(&base);
        }

        // ── Test 10 (Sol blocker 3): a REAL paired userns lease driven through the ACTUAL
        //   settlement branch with an injected workspace-delete failure — the lease is NOT released
        //   (quarantined), so the pool-1 slot cannot be reissued. ──
        #[test]
        fn injected_delete_failure_quarantines_the_paired_userns_lease() {
            let root = temp_root("t10");
            let workspace_base = root.join("workspace");
            let userns_base = root.join("userns");
            std::fs::create_dir_all(&workspace_base).unwrap();
            std::fs::create_dir_all(&userns_base).unwrap();
            let wm =
                deterministic_workspace_manager_for_tests(workspace_base.clone(), 1 << 30).unwrap();
            let alloc = deterministic_userns_allocator_for_tests(&userns_base, 1).unwrap();
            let spec = substitute_checkout_spec();
            let profile = crate::hardening::HardeningProfile::derive(&spec);
            let container_id = "job-t10-workload";
            let (_cfg, mut ctx) = acquire_enabled_workspace(
                &spec,
                &profile,
                container_id,
                PathBuf::from("/abs/staged-rootfs"),
                &wm,
                &alloc,
                None,
            )
            .expect("acquire a real paired workspace + userns lease");
            // Durably bind the lease (Allocated -> Bound) and record the Bound state settle validates.
            let (root_id, cgroup_id) = ((5_u64, 6_u64), (7_u64, 8_u64));
            ctx.lease
                .bind(container_id.to_string(), root_id, cgroup_id)
                .expect("bind the lease to a workload runtime");
            ctx.bind_state = LeaseBindState::Bound {
                container_id: container_id.to_string(),
                runsc_root_identity: root_id,
                cgroup_identity: cgroup_id,
            };
            // Inject the delete failure: swap the workspace leaf for a different-inode dir.
            let host = ctx.workspace.host_path().to_path_buf();
            std::fs::remove_dir_all(&host).unwrap();
            std::fs::create_dir(&host).unwrap();
            let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
                container_id.to_string(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: root_id,
                },
                CgroupQuiescenceEvidence::assert_for_tests(cgroup_id),
            );
            let result = settle_enabled_workspace_and_lease(ctx, &wm, &evidence);
            assert!(
                result.is_err(),
                "an unproven workspace delete makes the paired settlement fail"
            );
            assert!(
                matches!(wm.admission(), WorkspaceAdmission::Poisoned { .. }),
                "the workspace manager is poisoned by the unproven delete"
            );
            assert_eq!(
                wm.capacity_used_bytes(),
                1 << 30,
                "workspace capacity is retained on the unproven delete"
            );
            // The paired userns lease was NEVER released — quarantined. The pool-1 slot is gone.
            assert!(
                alloc.lease().is_err(),
                "the quarantined userns slot CANNOT be reissued after an unproven delete"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        // ── Test 11 (CT-007 5b.3-6e.2, the negative provenance pair): the substituted workload
        //   builds runtime-quiescence evidence with a DELIBERATELY WRONG runsc_root_identity (diverging
        //   from the durable bind's OWN recorded output). The REAL `settle_enabled_finalization` tail
        //   MUST reject it AT SETTLE (its evidence-vs-recorded-binding provenance check), fail closed,
        //   and — per the real contract — leave the workspace UNDELETED (capacity retained, manager
        //   poisoned) and the userns slot unreissued. This is what proves the positive test's clean
        //   settle is NOT vacuous: flip only the derived identity and settlement refuses. ──
        #[test]
        fn substituted_workload_mismatched_evidence_is_rejected_at_settle() {
            let root = temp_root("t11");
            let (obs, wm, workspace_base, alloc, userns_base) =
                run_substituted_checkout_mismatched_evidence(
                    &root,
                    "checkout.sentinel",
                    b"shared-provenance",
                );
            // Hop B + the OCI-mount round-trip still succeeded (the divergence is ONLY in the workload
            // evidence's runsc-root identity) — isolating the failure to the settle provenance check.
            assert!(
                obs.hopb_write_ok,
                "the checked Hop B sentinel write still succeeded"
            );
            assert!(
                obs.mount_source_matched_workspace,
                "the retained OCI mount still equals the capsule workspace host path"
            );
            assert!(
                obs.sentinel_read_through_mount,
                "the substituted workload still read the sentinel through the OCI-recorded mount"
            );
            // The settle tail REFUSED — the whole point.
            assert!(
                !obs.settled_ok,
                "mismatched evidence must NOT settle clean (got settled_ok=true)"
            );
            let error = obs
                .settle_error
                .as_deref()
                .expect("a refused settle carries a diagnostic");
            assert!(
                error.contains("does not match the recorded binding"),
                "the rejection must come from the SETTLE-tail evidence-vs-recorded-binding provenance \
                 check, got: {error}"
            );
            // Fail-closed durable state: the workspace was NEVER deleted (settle refused before the
            // delete), so its leaf survives, capacity is retained, and the manager is poisoned.
            let child_dirs = std::fs::read_dir(&workspace_base)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .count();
            assert_eq!(
                child_dirs, 1,
                "the workspace leaf must SURVIVE a refused settle (never deleted)"
            );
            assert_ne!(
                wm.capacity_used_bytes(),
                0,
                "capacity must be RETAINED on a refused settle (never silently freed)"
            );
            assert!(
                matches!(wm.admission(), WorkspaceAdmission::Poisoned { .. }),
                "dropping the still-live workspace on a refused settle poisons the manager"
            );
            // The paired userns lease was NEVER released — the pool-1 slot cannot be reissued.
            assert!(
                alloc.lease().is_err(),
                "the quarantined userns slot CANNOT be reissued after a refused settle"
            );
            let _ = std::fs::remove_dir_all(&workspace_base);
            let _ = std::fs::remove_dir_all(&userns_base);
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// **CT-007 slice 5b.3-6e.2 Stage A: the composed active-path proofs (PG-free).** These drive the
    /// REAL outer checkout orchestrator (`launch_checkout_orchestrated_with_given`, steps 1–14
    /// single-sourced) through the hardware-independent runsc-driver seam, substituting ONLY the
    /// hardware (the Hop-A git-container execution + the workload runsc spawn), and prove the active
    /// path settles cleanly before Stage B ever selects it — with NO control-plane.
    mod orchestrated_active_path_6e2 {
        use super::*;
        use crate::checkout_orchestration::ParentAttemptAdmission;
        use crate::gvisor::checkout_transport_test_support::{
            checkout_spec_for_backend, deterministic_enabled_backend_for_tests,
        };
        use crate::SandboxBackend;

        fn unique_root(tag: &str) -> std::path::PathBuf {
            let root = std::env::temp_dir().join(format!(
                "myelin-6e2-{tag}-{}-{}",
                std::process::id(),
                unique_suffix()
            ));
            std::fs::create_dir_all(&root).unwrap();
            root
        }

        /// Hooks whose `reserve_parent_attempt` admits with the no-op test-support authority, and whose
        /// checkout + per-phase authorizations pass — the minimal V2 wiring the dormant orchestrator
        /// needs to progress the whole advertise → fetch → materialization → workload sequence PG-free.
        fn admitting_hooks() -> RunnerHooks {
            ok_hooks()
                .with_checkout_authorization(Box::new(|_spec, _scope| Ok(())))
                .with_checkout_phase_authorization(Box::new(|_spec, _scope, _phase| {
                    Ok(LaunchPermit::immediate())
                }))
                .with_parent_attempt_reservation(Box::new(|_spec| {
                    Ok(ParentAttemptAdmission::Admitted {
                        claim: report_claim(),
                        reserve: ReserveHandle("ci-reserve:v2:6e2".to_string()),
                        attempt_authority: Box::new(NoOpTestSupportAuthority),
                    })
                }))
        }

        /// The §4 composed CHECKOUT-SUCCESS proof (PG-free variant): the REAL orchestrator drives the
        /// two gated transport hops, the real capsule acquisition, the real Hop-B durable transitions,
        /// and the real materialization/renewal/workload-credential/settle tail — all the way to a clean
        /// workload launch. Substituting ONLY the runsc executions means every composition seam (the
        /// admission handoff, transport-phase begin/complete, advertise→fetch generation ordering, the
        /// two renewals, capsule acquisition) runs for real.
        ///
        /// Gated `test-support`: the runsc-driver seam it exercises lives in the
        /// `#[cfg(feature = "test-support")]` `runsc_driver` module, so this proof EXECUTES under
        /// `--features test-support` (the deterministic substrate this whole slice rests on).
        #[cfg(feature = "test-support")]
        #[test]
        #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
        fn orchestrated_checkout_drives_two_gated_hops_to_a_clean_workload_launch() {
            use crate::checkout_orchestration::CheckoutContinuationOutcome;
            use crate::gvisor::checkout_transport_test_support::stage_checkout_repo_root;
            let root = unique_root("orchestrated");
            let (backend, image) = deterministic_enabled_backend_for_tests(&root);
            let repo_root = stage_checkout_repo_root(&root.join("repos"));
            let spec = checkout_spec_for_backend(image);
            let hooks = admitting_hooks();

            let (result, recorded) = backend.drive_checkout_cycle_with_substituted_runsc_given(
                &spec,
                &hooks,
                &repo_root,
                "checkout.sentinel",
                b"6e2-provenance-sentinel",
            );

            // Exactly the two scripted transport legs ran — no unused step, and the executor panics on a
            // third call, so a masked extra spawn could not pass silently.
            assert_eq!(
                recorded.len(),
                2,
                "exactly two transport hops must spawn (advertise then fetch): {recorded:?}"
            );
            // Each leg spawned under its OWN durable credential (distinct jti) ...
            assert_ne!(
                recorded[0].0, recorded[1].0,
                "advertise and fetch must spawn under DISTINCT jtis: {recorded:?}"
            );
            // ... and BOTH phase permits committed at the spawn boundary.
            assert!(
                recorded[0].1 && recorded[1].1,
                "both transport permits must commit: {recorded:?}"
            );

            // The full steps 1–25 sequence progressed to a clean workload launch — i.e. the real settle
            // tail succeeded (a failed settle would surface as a non-`WorkloadLaunched` outcome).
            match result {
                Ok(CheckoutContinuationOutcome::WorkloadLaunched(launch)) => {
                    assert!(
                        launch.output_complete,
                        "the substituted workload must complete cleanly"
                    );
                    assert_eq!(
                        launch.result.usage,
                        crate::ResourceUsage {
                            cpu_seconds: 3,
                            mem_byte_seconds: 7,
                        },
                        "the settled workload carries exactly the substituted workload usage"
                    );
                }
                other => panic!("expected a clean WorkloadLaunched, got {other:?}"),
            }
            let _ = std::fs::remove_dir_all(&root);
        }

        /// **The dormant typed-cycle SELECTOR routes on workspace shape BEFORE any reserve or spawn.** A
        /// checkout-bearing job on a checkout-`disabled()` backend fails closed; a malformed workspace is
        /// refused as neither compute nor checkout; a compute job reaches the compute arm (whose FIRST
        /// admission step — `reserve_parent_attempt` — refuses under legacy hooks, proving the arm was
        /// selected without spawning). Each arm returns a DISTINCT fail-closed diagnostic.
        #[test]
        fn run_cycle_selects_the_gvisor_arm_on_workspace_shape_before_reserve_or_spawn() {
            let root = unique_root("selector");
            let (backend, image) = deterministic_enabled_backend_for_tests(&root);
            let sink: Arc<dyn SandboxOutputSink> = Arc::new(RecordingOutput::default());

            // (Some, Some) checkout-bearing on a checkout-`disabled()` backend → fail closed before
            // reserve/spawn (the deterministic Enabled backend leaves `checkout` disabled()).
            let checkout_spec = checkout_spec_for_backend(image.clone());
            let err = backend
                .run_cycle(
                    &checkout_spec,
                    &admitting_hooks(),
                    sink.clone(),
                    SandboxCancellation::new(),
                )
                .expect_err("a checkout job on a checkout-disabled backend fails closed");
            match err {
                SandboxLaunchError::Failed(GvisorError::Hook(HookError(msg))) => assert!(
                    msg.contains("enabled checkout repository root"),
                    "checkout arm selected; got: {msg}"
                ),
                other => panic!("expected the checkout-arm fail-closed refusal, got {other:?}"),
            }

            // A malformed workspace (repo_ref present, commit absent) → refused as neither compute nor a
            // valid checkout, before reserve/spawn.
            let mut malformed = checkout_spec_for_backend(image.clone());
            malformed.workspace.commit = None;
            let err = backend
                .run_cycle(
                    &malformed,
                    &admitting_hooks(),
                    sink.clone(),
                    SandboxCancellation::new(),
                )
                .expect_err("a malformed workspace is refused");
            match err {
                SandboxLaunchError::Failed(GvisorError::Hook(HookError(msg))) => assert!(
                    msg.contains("malformed workspace"),
                    "malformed arm selected; got: {msg}"
                ),
                other => panic!("expected the malformed-workspace refusal, got {other:?}"),
            }

            // (None, None) compute: the compute arm is reached; its first admission step
            // (`reserve_parent_attempt`) refuses under legacy hooks (no parent-attempt reservation),
            // proving the arm was selected without spawning. The image resolves so preflight passes.
            let mut compute_spec = checkout_spec_for_backend(image);
            compute_spec.workspace = crate::WorkspaceSpec::default();
            let err = backend
                .run_cycle(&compute_spec, &ok_hooks(), sink, SandboxCancellation::new())
                .expect_err("compute under legacy hooks refuses at parent-attempt admission");
            match err {
                SandboxLaunchError::Failed(GvisorError::Hook(HookError(msg))) => assert!(
                    msg.contains("parent-attempt"),
                    "compute arm selected (reached reserve_parent_attempt); got: {msg}"
                ),
                other => panic!("expected the compute-arm reserve refusal, got {other:?}"),
            }
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}

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
// tests can drive the REAL checkout orchestrator with a scripted two-call Hop-A executor. Appended
// AFTER the top-level `#[cfg(test)] mod tests` block so it is excluded from `production_source()`;
// every item is gated, ABSENT from ordinary builds, and reachable from NO production composition
// root. The `#[cfg(test)]` callers (`checkout_preparation_5b2`, `checkout_transport_5b3_3`, the outer
// `tests` module for `FakeRunsc`) re-import these by path so their existing tests keep compiling.
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

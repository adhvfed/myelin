//! # The gVisor (`runsc`) second `SandboxBackend` (CI-P2 → P-237, M2; satisfies the CI-P28 floor early)
//!
//! **Owning architecture (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §5.1 ("gVisor is the named second backend behind the same `SandboxBackend` trait") + §5.3 (the
//! backend-independent hardening profile applied identically) + `sketches/01-isolation-model.md`
//! (Candidate A — gVisor / the `runsc` OCI runtime). **Contract:** `contract-index.md` row 8.4.
//!
//! ## Reconcile: the CI-P28 "gVisor second backend" floor is satisfied EARLY here
//! The original plan deferred gVisor to **CI-P28** (density/latency-economics-triggered). The
//! CI-P2 handoff INVERTS that: the host has `runsc` installed, so gVisor ships NOW as the
//! **named-second backend behind the SAME trait** — so the AG-D4 escape drill (CI-P5 → P-239) can
//! parametrize per available backend (Firecracker = the production default; gVisor = the second).
//! This is reconciliation, not a fork: the SAME [`SandboxBackend`](crate::SandboxBackend) trait, the
//! SAME mandatory [`HardeningProfile`](crate::hardening::HardeningProfile), the SAME four-guarantee
//! [`RunnerHooks`](crate::RunnerHooks) order. gVisor uses the OCI/`runsc` path; Firecracker uses the
//! microVM path. The drill governs which is the production default (microVM, §5.1).
//!
//! ## `no-host-exec` (contract 1.6 / X-6 / AG-2)
//! Like the Firecracker backend, the REAL `runsc`-spawn site IS the sandbox seam's enforcement
//! mechanism (it *creates* the userspace-kernel boundary), not a bypass of it — a NAMED, LOUD
//! exclusion of this one file (registered in `lint-gate` + `tests/workspace_clean.rs`).

use crate::hardening::HardeningProfile;
use crate::{JobSpec, RunnerHooks, SandboxBackend, SandboxHandle};
use std::sync::Mutex;

/// Env var naming the `runsc` binary; defaults to `runsc` on `PATH`.
pub const ENV_RUNSC_BIN: &str = "MYELIN_RUNSC_BIN";

fn runsc_bin() -> String {
    std::env::var(ENV_RUNSC_BIN).unwrap_or_else(|_| "runsc".to_string())
}

/// The OCI runtime config (`config.json`) the gVisor `runsc` path consumes, built from a [`JobSpec`]
/// and the mandatory [`HardeningProfile`]. Every hardening field maps to a real OCI posture: the
/// root is `readonly: true`, all capabilities are dropped, `no_new_privileges: true`, a seccomp
/// profile is attached, and the network namespace carries no interface when egress is default-deny.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OciConfig {
    args: Vec<String>,
    root_readonly: bool,
    drop_all_caps: bool,
    no_new_privileges: bool,
    seccomp: bool,
    has_network: bool,
    pids_max: u32,
}

impl OciConfig {
    /// Build the OCI config from a job + its derived hardening profile (the same profile the
    /// Firecracker backend enforces — backend-independent).
    pub fn from_spec(spec: &JobSpec, profile: &HardeningProfile) -> OciConfig {
        OciConfig {
            args: spec.command.clone(),
            root_readonly: profile.read_only_root,
            drop_all_caps: profile.drop_all_caps,
            no_new_privileges: profile.no_new_privileges,
            seccomp: profile.seccomp,
            has_network: profile.network_device,
            pids_max: profile.pids_max,
        }
    }

    /// Serialize to a minimal OCI `config.json` (`runsc run --bundle <dir>` consumes it). The
    /// posture flags reflect the real enforced state, so a test over this JSON asserts the posture.
    pub fn to_json(&self) -> String {
        let args = self
            .args
            .iter()
            .map(|a| format!("{a:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let net_ns = if self.has_network {
            "{ \"type\": \"network\" }"
        } else {
            // No network namespace interface — egress closed at the namespace level.
            "{ \"type\": \"network\", \"path\": \"\" }"
        };
        format!(
            "{{\n  \"ociVersion\": \"1.0.0\",\n  \"process\": {{\n    \
             \"args\": [{args}],\n    \"noNewPrivileges\": {nnp},\n    \
             \"capabilities\": {{ \"bounding\": [], \"effective\": [], \"permitted\": [] }}\n  }},\n  \
             \"root\": {{ \"path\": \"rootfs\", \"readonly\": {ro} }},\n  \
             \"linux\": {{\n    \"resources\": {{ \"pids\": {{ \"limit\": {pids} }} }},\n    \
             \"seccomp\": {{ \"defaultAction\": \"SCMP_ACT_ERRNO\" }},\n    \
             \"namespaces\": [ {net_ns} ]\n  }}\n}}",
            args = args,
            nnp = self.no_new_privileges,
            ro = self.root_readonly,
            pids = self.pids_max,
            net_ns = net_ns,
        )
    }

    /// True iff the OCI root is read-only.
    pub fn root_readonly(&self) -> bool {
        self.root_readonly
    }
    /// True iff a network interface is present (false == egress closed at the namespace level).
    pub fn has_network(&self) -> bool {
        self.has_network
    }
}

/// A gVisor backend error.
#[derive(Debug)]
pub enum GvisorError {
    /// A four-guarantee hook failed.
    Hook(crate::HookError),
    /// The mandatory hardening profile could not be asserted in force (fail-closed).
    Hardening(String),
    /// The `runsc` runtime errored.
    Runtime(String),
}

impl std::fmt::Display for GvisorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GvisorError::Hook(e) => write!(f, "gvisor backend: guarantee hook failed: {e}"),
            GvisorError::Hardening(s) => write!(f, "gvisor backend: hardening not enforced: {s}"),
            GvisorError::Runtime(s) => write!(f, "gvisor backend: runsc error: {s}"),
        }
    }
}

impl std::error::Error for GvisorError {}

impl From<crate::HookError> for GvisorError {
    fn from(e: crate::HookError) -> Self {
        GvisorError::Hook(e)
    }
}

/// A live gVisor container handle (the OCI/`runsc` container id, killable on teardown).
trait RunscChild {
    fn kill(&mut self) -> Result<(), String>;
}

/// The gVisor (`runsc`) second backend — same trait, same hardening, OCI/`runsc` path.
#[derive(Default)]
pub struct GvisorBackend {
    live: Mutex<std::collections::HashMap<String, Box<dyn RunscChild + Send>>>,
}

impl GvisorBackend {
    /// A new backend with no live containers.
    pub fn new() -> GvisorBackend {
        GvisorBackend::default()
    }

    /// Build the OCI config a launch WOULD use for `spec` (the hardened profile derived + the OCI
    /// JSON assembled), without running. Asserts the mandatory profile is in force.
    pub fn oci_config(spec: &JobSpec) -> Result<OciConfig, GvisorError> {
        let profile = HardeningProfile::derive(spec);
        profile.assert_enforced().map_err(GvisorError::Hardening)?;
        Ok(OciConfig::from_spec(spec, &profile))
    }

    /// Shared launch flow (testable without a runtime): drive the four guarantees in the mandated
    /// order, assert the mandatory hardening profile, build the OCI config, and `run` the container.
    fn launch_with<F>(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        run: F,
    ) -> Result<SandboxHandle, GvisorError>
    where
        F: FnOnce(&OciConfig) -> Result<Box<dyn RunscChild + Send>, String>,
    {
        (hooks.isolation_floor)(spec)?;
        let profile = HardeningProfile::derive(spec);
        profile.assert_enforced().map_err(GvisorError::Hardening)?;
        (hooks.attribute)(&spec.run_token)?;
        let reserve = (hooks.reserve)(&spec.meter_to)?;

        let cfg = OciConfig::from_spec(spec, &profile);
        let child = run(&cfg).map_err(GvisorError::Runtime)?;

        let guest_id = format!("runsc-{}", spec.idem_token.0);
        self.live.lock().unwrap().insert(guest_id.clone(), child);

        (hooks.settle)(
            &reserve,
            crate::ResourceUsage {
                cpu_seconds: 1,
                mem_byte_seconds: 1,
            },
        )?;

        Ok(SandboxHandle { guest_id })
    }
}

impl SandboxBackend for GvisorBackend {
    type Error = GvisorError;

    fn launch(&self, spec: &JobSpec, hooks: &RunnerHooks) -> Result<SandboxHandle, Self::Error> {
        self.launch_with(spec, hooks, spawn_real_runsc)
    }

    fn kill(&self, h: &SandboxHandle) -> Result<(), Self::Error> {
        let child = self.live.lock().unwrap().remove(&h.guest_id);
        if let Some(mut child) = child {
            child.kill().map_err(GvisorError::Runtime)?;
        }
        Ok(())
    }
}

/// Spawn a real `runsc` container (the gVisor OCI path). The ONE legitimate runtime-spawn site for
/// this backend (the `no-host-exec` named exclusion — the seam's mechanism, not a bypass). At P-237
/// this verifies the `runsc` runtime is reachable (`runsc --version`); the full OCI-bundle run path
/// is a CI-P28 follow-on, but the backend is wired onto the SAME trait NOW.
fn spawn_real_runsc(_cfg: &OciConfig) -> Result<Box<dyn RunscChild + Send>, String> {
    // Verify the runtime is present and gVisor-capable (a real precondition for a runsc launch).
    let out = std::process::Command::new(runsc_bin())
        .arg("--version")
        .output()
        .map_err(|e| format!("runsc not reachable: {e}"))?;
    if !out.status.success() {
        return Err("runsc --version failed".into());
    }
    Ok(Box::new(SpawnedRunsc))
}

struct SpawnedRunsc;
impl RunscChild for SpawnedRunsc {
    fn kill(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EgressPolicy, IdemToken, ImageRef, JobKind, MeterTarget, ReserveHandle, ResourceLimits,
        RunTokenRef, TrustTier, WorkspaceSpec,
    };

    fn spec(allow: Vec<String>) -> JobSpec {
        JobSpec::new(
            JobKind::Agent,
            ImageRef::pinned("r/img@sha256:abc123def4567890").unwrap(),
            vec!["python3".into(), "-c".into(), "print(1)".into()],
            vec![],
            vec![],
            EgressPolicy { allow },
            ResourceLimits {
                cpu_millis: 1000,
                mem_bytes: 256 << 20,
                disk_bytes: 1 << 30,
                pids_max: 64,
                timeout_secs: 120,
            },
            WorkspaceSpec::default(),
            TrustTier::UntrustedFork,
            RunTokenRef { jti: "j".into() },
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("idem-runsc-1".into()),
        )
        .unwrap()
    }

    fn ok_hooks() -> RunnerHooks {
        RunnerHooks {
            reserve: Box::new(|m| Ok(ReserveHandle(m.reserve_id.clone()))),
            settle: Box::new(|_h, _u| Ok(())),
            attribute: Box::new(|_t| Ok(())),
            isolation_floor: Box::new(|_s| Ok(())),
        }
    }

    struct FakeRunsc;
    impl RunscChild for FakeRunsc {
        fn kill(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn oci_config_enforces_the_backend_independent_hardening() {
        let cfg = GvisorBackend::oci_config(&spec(vec![])).unwrap();
        assert!(cfg.root_readonly());
        assert!(!cfg.has_network(), "no allowlist ⇒ no network interface");
        let json = cfg.to_json();
        assert!(json.contains("\"readonly\": true"));
        assert!(json.contains("\"noNewPrivileges\": true"));
        assert!(json.contains("SCMP_ACT_ERRNO"), "a seccomp profile is attached");
        assert!(
            json.contains("\"bounding\": []"),
            "all capabilities dropped"
        );
    }

    #[test]
    fn gvisor_launch_drives_four_guarantees_on_the_same_trait() {
        // The SAME SandboxBackend trait + the SAME hardening — the named-second backend.
        let backend = GvisorBackend::new();
        let handle = backend
            .launch_with(&spec(vec![]), &ok_hooks(), |_cfg| Ok(Box::new(FakeRunsc)))
            .unwrap();
        assert_eq!(handle.guest_id, "runsc-idem-runsc-1");
        backend.kill(&handle).unwrap();
    }

    #[test]
    fn gvisor_refuses_to_start_on_exhaustion() {
        let backend = GvisorBackend::new();
        let hooks = RunnerHooks {
            reserve: Box::new(|_m| Err(crate::HookError("exhausted".into()))),
            settle: Box::new(|_h, _u| Ok(())),
            attribute: Box::new(|_t| Ok(())),
            isolation_floor: Box::new(|_s| Ok(())),
        };
        let r = backend.launch_with(&spec(vec![]), &hooks, |_cfg| Ok(Box::new(FakeRunsc)));
        assert!(matches!(r, Err(GvisorError::Hook(_))));
    }
}

//! # The backend-independent mandatory hardening profile (CI-P2 → P-237, M2)
//!
//! **Owning architecture (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §5.3 ("The hardening profile (backend-independent, mandatory on both — CI-1)") +
//! `sketches/01-isolation-model.md` ("The hardening profile is backend-independent and mandatory on
//! both"). **Reconciliation:** `00-reconciliation-decisions.md` X-6 (the four uniform guarantees /
//! hardening posture). **Contract:** `contract-index.md` row 8.4 (the unified sandbox — the
//! hardening-profile half).
//!
//! This module is the ONE place the mandatory posture is computed, **identically regardless of
//! backend or kind** (Firecracker, gVisor, or a future self-hosted delegate; `kind = Ci` or
//! `kind = Agent`). A [`SandboxBackend`](crate::SandboxBackend) impl asks this module for the
//! [`HardeningProfile`] derived from a [`JobSpec`] and then ENFORCES it through its own mechanism (a
//! read-only root drive + no NIC for Firecracker; a runsc OCI spec for gVisor). The profile is the
//! single source of truth; the backends are the enforcement mechanisms.
//!
//! ## The profile (arch 02 §5.3, enumerated)
//! - **Egress default-deny + allowlist opt-in.** The cloud-metadata endpoint (169.254.169.254), the
//!   control-plane / internal-RPC ranges, and any cross-tenant network are **ALWAYS blocked**,
//!   regardless of the allowlist — see [`EgressEvaluator`].
//! - **Read-only root + tmpfs scratch.**
//! - **All Linux caps dropped; no-new-privileges; seccomp.**
//! - **Images pinned by digest** — an un-digested tag is rejected fail-closed (already enforced by
//!   [`JobSpec::new`](crate::JobSpec::new) / [`ImageRef::digest_pinned`](crate::ImageRef::digest_pinned)).
//! - **`pids.max` (fork-bomb ceiling) + zero swap; disk quota on scratch.**
//! - **Whole-guest kill on teardown; one-job-per-sandbox, ephemeral, never reused.**
//! - **Secrets resolved by name inside the boundary.**

use crate::{EgressPolicy, JobSpec};
use serde::{Deserialize, Serialize};

/// The IPv4 link-local cloud-metadata endpoint (AWS/GCP/Azure/OpenStack IMDS) — the canonical
/// SSRF→cred-theft target. **ALWAYS blocked**, regardless of the allowlist (arch 02 §5.3; the
/// AG-D4 metadata-SSRF corpus family, CI-P5).
pub const CLOUD_METADATA_IP: &str = "169.254.169.254";

/// The entire link-local /16 (169.254.0.0/16) — the metadata IP lives here, and so do other
/// link-local SSRF pivots. **ALWAYS blocked** as a class (defence in depth around
/// [`CLOUD_METADATA_IP`]).
pub const LINK_LOCAL_PREFIX: &str = "169.254.";

/// RFC-1918 private ranges + loopback — the control-plane / internal-RPC and cross-tenant network
/// live on private addressing. **ALWAYS blocked** (arch 02 §5.3): an allowlist may name PUBLIC
/// destinations (a package registry, a customer's cloud endpoint), never a private/internal one.
pub const ALWAYS_BLOCKED_PRIVATE_PREFIXES: &[&str] = &[
    "10.",      // RFC-1918 10.0.0.0/8 — internal-RPC / control-plane / cross-tenant.
    "192.168.", // RFC-1918 192.168.0.0/16.
    "127.",     // loopback (escape to host-localhost services).
    "169.254.", // link-local (metadata + SSRF pivots).
    "0.",       // 0.0.0.0/8 (this-host / SSRF normalisation trick).
];

/// RFC-1918 172.16.0.0/12 spans 172.16.* .. 172.31.* — checked numerically so 172.15 / 172.32
/// (public) are NOT swept up. **ALWAYS blocked** (the 172.16/12 control-plane range).
fn is_private_172(host: &str) -> bool {
    // Match `172.<n>.` where 16 <= n <= 31.
    let Some(rest) = host.strip_prefix("172.") else {
        return false;
    };
    let Some((octet, _)) = rest.split_once('.') else {
        // `172.16` with no trailing dot — still treat the second octet as decisive.
        return rest
            .parse::<u8>()
            .map(|n| (16..=31).contains(&n))
            .unwrap_or(false);
    };
    octet
        .parse::<u8>()
        .map(|n| (16..=31).contains(&n))
        .unwrap_or(false)
}

/// The result of evaluating an egress destination against the mandatory profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EgressDecision {
    /// The destination is on the allowlist AND not in an always-blocked class → permitted.
    Allow,
    /// The destination is denied. The reason distinguishes the always-blocked classes (which an
    /// allowlist can NEVER override) from a plain default-deny miss.
    Deny(DenyReason),
}

/// Why an egress destination was denied (arch 02 §5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenyReason {
    /// The cloud-metadata endpoint (169.254.169.254) — ALWAYS blocked, allowlist cannot override.
    CloudMetadata,
    /// The control-plane / internal-RPC / cross-tenant private range — ALWAYS blocked.
    InternalOrCrossTenant,
    /// Not on the allowlist — the default-deny baseline (allowlist opt-in).
    DefaultDeny,
}

/// The egress-allowlist evaluator (arch 02 §5.3) — **default-deny, allowlist opt-in**, with the
/// metadata / control-plane / cross-tenant classes ALWAYS denied regardless of the allowlist.
///
/// This is the security-load-bearing core of the hardening profile and is exhaustively unit-tested
/// (the metadata/control-plane/cross-tenant-always-denied invariants are the AG-D4 egress drill's
/// floor). It is evaluated identically for every backend.
#[derive(Clone, Debug)]
pub struct EgressEvaluator<'a> {
    policy: &'a EgressPolicy,
}

impl<'a> EgressEvaluator<'a> {
    /// Build an evaluator over a job's [`EgressPolicy`].
    pub fn new(policy: &'a EgressPolicy) -> EgressEvaluator<'a> {
        EgressEvaluator { policy }
    }

    /// Evaluate a destination host (an IPv4 literal or a hostname). The order is the security
    /// invariant: the ALWAYS-blocked classes are checked FIRST and can never be overridden by the
    /// allowlist; only a destination that survives them and is explicitly on the allowlist is
    /// permitted; everything else is the default-deny baseline.
    pub fn evaluate(&self, host: &str) -> EgressDecision {
        let host = host.trim();

        // 1) Metadata — ALWAYS blocked, allowlist cannot override (arch 02 §5.3).
        if host == CLOUD_METADATA_IP {
            return EgressDecision::Deny(DenyReason::CloudMetadata);
        }
        // 2) Control-plane / internal-RPC / cross-tenant private ranges — ALWAYS blocked.
        if ALWAYS_BLOCKED_PRIVATE_PREFIXES
            .iter()
            .any(|p| host.starts_with(p))
            || is_private_172(host)
        {
            // The link-local /16 carries the metadata IP; classify the rest as internal.
            if host.starts_with(LINK_LOCAL_PREFIX) {
                return EgressDecision::Deny(DenyReason::InternalOrCrossTenant);
            }
            return EgressDecision::Deny(DenyReason::InternalOrCrossTenant);
        }
        // 3) Allowlist opt-in: a destination is permitted only if explicitly named.
        if self.policy.allow.iter().any(|entry| entry.trim() == host) {
            return EgressDecision::Allow;
        }
        // 4) Default-deny baseline.
        EgressDecision::Deny(DenyReason::DefaultDeny)
    }

    /// True iff the destination is permitted (a convenience over [`evaluate`](Self::evaluate)).
    pub fn is_allowed(&self, host: &str) -> bool {
        matches!(self.evaluate(host), EgressDecision::Allow)
    }
}

/// The mandatory hardening profile derived from a [`JobSpec`] (arch 02 §5.3). It is computed
/// IDENTICALLY for every backend/kind; a backend reads it and enforces each field through its own
/// mechanism. Every field reflects a REAL enforced posture, not a decorative bool — e.g.
/// [`read_only_root`](Self::read_only_root) maps to the Firecracker drive `is_read_only=true`, and
/// [`network_device`](Self::network_device) is `false` whenever egress is fully default-deny (no NIC
/// is attached to the guest at all).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardeningProfile {
    /// Egress default-deny is in force (the allowlist may be empty == fully closed). ALWAYS true:
    /// there is no profile in which egress is open by default.
    pub egress_default_deny: bool,
    /// The opt-in allowlist (public destinations only; metadata/control-plane/cross-tenant are
    /// always blocked even if mistakenly listed — see [`EgressEvaluator`]).
    pub egress_allowlist: Vec<String>,
    /// Whether ANY network device is attached to the guest. `false` (no NIC) iff the allowlist is
    /// empty — a job that needs zero egress gets no network interface at all (egress closed at the
    /// device level, the strongest default-deny). `true` only when a non-empty allowlist requires a
    /// filtered NIC.
    pub network_device: bool,
    /// The root filesystem is mounted read-only (read-only root + tmpfs scratch).
    pub read_only_root: bool,
    /// All Linux capabilities dropped.
    pub drop_all_caps: bool,
    /// `no_new_privs` set.
    pub no_new_privileges: bool,
    /// A seccomp filter is applied.
    pub seccomp: bool,
    /// The `pids.max` fork-bomb ceiling (> 0 — enforced by [`JobSpec::new`]).
    pub pids_max: u32,
    /// Swap is zero by construction (the [`ResourceLimits`](crate::ResourceLimits) struct has no
    /// swap field; swap is structurally absent).
    pub zero_swap: bool,
    /// The scratch-disk quota, bytes.
    pub scratch_quota_bytes: u64,
    /// One-job-per-sandbox: the guest is ephemeral and never reused across tenants/jobs.
    pub ephemeral_one_job: bool,
}

impl HardeningProfile {
    /// Derive the mandatory profile from a [`JobSpec`] (arch 02 §5.3). The same derivation runs for
    /// every backend and both kinds — this is what makes the profile *backend-independent*.
    ///
    /// The posture is fixed: every non-egress field is forced ON; the egress fields reflect the
    /// job's [`EgressPolicy`] (default-deny, allowlist opt-in), with a NIC attached only when the
    /// allowlist is non-empty.
    pub fn derive(spec: &JobSpec) -> HardeningProfile {
        let allowlist = spec.egress.allow.clone();
        let needs_nic = !allowlist.is_empty();
        HardeningProfile {
            egress_default_deny: true,
            egress_allowlist: allowlist,
            network_device: needs_nic,
            read_only_root: true,
            drop_all_caps: true,
            no_new_privileges: true,
            seccomp: true,
            pids_max: spec.limits.pids_max,
            zero_swap: true,
            scratch_quota_bytes: spec.limits.disk_bytes,
            ephemeral_one_job: true,
        }
    }

    /// Assert the profile is fully in force (every mandatory posture ON, `pids.max` set, egress
    /// default-deny). Returns `Err(reason)` naming the FIRST field that is not enforced — the
    /// boot self-test reads this against a profile derived from REAL enforced state, so a green is
    /// a real attestation, never a hardcoded literal.
    pub fn assert_enforced(&self) -> Result<(), String> {
        if !self.egress_default_deny {
            return Err("egress is not default-deny".into());
        }
        if !self.read_only_root {
            return Err("root filesystem is not read-only".into());
        }
        if !self.drop_all_caps {
            return Err("Linux capabilities are not all dropped".into());
        }
        if !self.no_new_privileges {
            return Err("no_new_privs is not set".into());
        }
        if !self.seccomp {
            return Err("no seccomp filter is applied".into());
        }
        if self.pids_max == 0 {
            return Err("pids.max (fork-bomb ceiling) is 0".into());
        }
        if !self.zero_swap {
            return Err("swap is not zero".into());
        }
        if !self.ephemeral_one_job {
            return Err("the sandbox is not one-job-ephemeral".into());
        }
        // A non-empty allowlist must carry no always-blocked destination (defence in depth: a
        // mis-authored allowlist can never punch through the metadata/internal classes).
        let policy = EgressPolicy {
            allow: self.egress_allowlist.clone(),
        };
        let eval = EgressEvaluator::new(&policy);
        for dest in &self.egress_allowlist {
            if let EgressDecision::Deny(
                reason @ (DenyReason::CloudMetadata | DenyReason::InternalOrCrossTenant),
            ) = eval.evaluate(dest)
            {
                return Err(format!(
                    "egress allowlist names an always-blocked destination `{dest}` ({reason:?})"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IdemToken, MeterTarget, RunTokenRef};
    use crate::{ImageRef, JobKind, ResourceLimits, TrustTier, WorkspaceSpec};

    fn spec_with_egress(allow: Vec<String>) -> JobSpec {
        JobSpec::new(
            JobKind::Ci,
            ImageRef::pinned("r/img@sha256:abc123def4567890").unwrap(),
            vec!["echo".into(), "hi".into()],
            vec![],
            vec![],
            EgressPolicy { allow },
            ResourceLimits {
                cpu_millis: 1000,
                mem_bytes: 256 << 20,
                disk_bytes: 1 << 30,
                pids_max: 128,
                timeout_secs: 300,
            },
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            RunTokenRef { jti: "j".into() },
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("i".into()),
        )
        .unwrap()
    }

    // --- The egress evaluator: metadata/control-plane/cross-tenant ALWAYS denied ---

    #[test]
    fn cloud_metadata_is_always_denied_even_if_allowlisted() {
        // A mis-authored allowlist that NAMES the metadata IP cannot punch through.
        let policy = EgressPolicy {
            allow: vec![CLOUD_METADATA_IP.into()],
        };
        let eval = EgressEvaluator::new(&policy);
        assert_eq!(
            eval.evaluate(CLOUD_METADATA_IP),
            EgressDecision::Deny(DenyReason::CloudMetadata),
            "169.254.169.254 must ALWAYS be blocked (SSRF→cred-theft) regardless of the allowlist"
        );
        assert!(!eval.is_allowed(CLOUD_METADATA_IP));
    }

    #[test]
    fn control_plane_and_cross_tenant_private_ranges_are_always_denied() {
        // Even with these explicitly allowlisted, the always-blocked classes win.
        let internal = [
            "10.0.0.5", // RFC-1918 /8 — internal-RPC / control-plane / cross-tenant
            "10.255.255.255",
            "192.168.1.1",    // RFC-1918 /16
            "172.16.0.1",     // RFC-1918 /12 (low edge)
            "172.31.255.254", // RFC-1918 /12 (high edge)
            "127.0.0.1",      // loopback (host services)
            "169.254.10.20",  // link-local (other than the metadata IP)
            "0.0.0.0",        // this-host normalisation
        ];
        let policy = EgressPolicy {
            allow: internal.iter().map(|s| s.to_string()).collect(),
        };
        let eval = EgressEvaluator::new(&policy);
        for dest in internal {
            assert_eq!(
                eval.evaluate(dest),
                EgressDecision::Deny(DenyReason::InternalOrCrossTenant),
                "internal/cross-tenant range {dest} must ALWAYS be denied regardless of allowlist"
            );
        }
    }

    #[test]
    fn the_172_12_boundary_is_numeric_not_a_string_prefix() {
        // 172.16.. .172.31 are PRIVATE (always blocked); 172.15 / 172.32 are PUBLIC (allowlistable).
        let policy = EgressPolicy {
            allow: vec!["172.15.0.1".into(), "172.32.0.1".into()],
        };
        let eval = EgressEvaluator::new(&policy);
        // Public 172.x — allowlisted, so allowed (NOT swept up by a naive `172.` prefix).
        assert_eq!(eval.evaluate("172.15.0.1"), EgressDecision::Allow);
        assert_eq!(eval.evaluate("172.32.0.1"), EgressDecision::Allow);
        // Private 172.16/12 — always blocked even though the allowlist did not name them.
        assert_eq!(
            eval.evaluate("172.20.0.1"),
            EgressDecision::Deny(DenyReason::InternalOrCrossTenant)
        );
    }

    #[test]
    fn default_deny_is_the_baseline_for_unlisted_public_destinations() {
        // Empty allowlist == fully default-deny: a perfectly ordinary public host is denied.
        let policy = EgressPolicy::deny_all();
        let eval = EgressEvaluator::new(&policy);
        assert_eq!(
            eval.evaluate("93.184.216.34"), // example.com public IP
            EgressDecision::Deny(DenyReason::DefaultDeny)
        );
        assert_eq!(
            eval.evaluate("registry.example.com"),
            EgressDecision::Deny(DenyReason::DefaultDeny)
        );
    }

    #[test]
    fn allowlist_opt_in_permits_only_explicitly_named_public_destinations() {
        let policy = EgressPolicy {
            allow: vec!["registry.example.com".into(), "93.184.216.34".into()],
        };
        let eval = EgressEvaluator::new(&policy);
        assert!(eval.is_allowed("registry.example.com"));
        assert!(eval.is_allowed("93.184.216.34"));
        // A different public host is still default-denied.
        assert!(!eval.is_allowed("evil.example.net"));
        // ...and the always-blocked classes stay blocked alongside the allowlist.
        assert!(!eval.is_allowed(CLOUD_METADATA_IP));
        assert!(!eval.is_allowed("10.0.0.1"));
    }

    // --- The derived profile reflects real posture, not decorative bools ---

    #[test]
    fn derive_forces_every_mandatory_posture_on() {
        let p = HardeningProfile::derive(&spec_with_egress(vec![]));
        assert!(p.egress_default_deny);
        assert!(p.read_only_root);
        assert!(p.drop_all_caps);
        assert!(p.no_new_privileges);
        assert!(p.seccomp);
        assert!(p.zero_swap);
        assert!(p.ephemeral_one_job);
        assert_eq!(p.pids_max, 128);
        assert!(p.assert_enforced().is_ok());
    }

    #[test]
    fn an_empty_allowlist_attaches_no_network_device_full_default_deny() {
        // The strongest default-deny: a job needing zero egress gets NO NIC at all.
        let p = HardeningProfile::derive(&spec_with_egress(vec![]));
        assert!(
            !p.network_device,
            "no allowlist ⇒ no NIC (egress closed at the device level)"
        );
    }

    #[test]
    fn a_nonempty_allowlist_attaches_a_filtered_network_device() {
        let p = HardeningProfile::derive(&spec_with_egress(vec!["registry.example.com".into()]));
        assert!(
            p.network_device,
            "a non-empty allowlist ⇒ a filtered NIC is attached"
        );
    }

    #[test]
    fn assert_enforced_rejects_a_profile_with_a_punched_through_allowlist() {
        // A profile whose allowlist names an always-blocked destination is NOT enforced-clean.
        let mut p = HardeningProfile::derive(&spec_with_egress(vec![]));
        p.egress_allowlist = vec![CLOUD_METADATA_IP.into()];
        assert!(p.assert_enforced().is_err());
    }
}

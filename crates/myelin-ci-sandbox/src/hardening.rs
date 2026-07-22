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

/// The host-side tap device the Firecracker egress NIC is wired to (`network-interfaces[].host_dev_name`
/// in `firecracker.rs`). The R0.1 egress firewall ruleset filters traffic **ingressing the host from
/// this interface** (the guest's egress), so the interface name here MUST match the one the machine
/// config attaches — they are one indivisible thing.
pub const EGRESS_TAP_DEVICE: &str = "tap-myelin";

/// The nftables CIDR literals for the always-blocked egress classes — the **firewall-enforceable
/// form** of [`CLOUD_METADATA_IP`] + [`LINK_LOCAL_PREFIX`] + [`ALWAYS_BLOCKED_PRIVATE_PREFIXES`] +
/// the 172.16/12 range [`is_private_172`] matches numerically. This list is kept BESIDE the classifier
/// constants (and cross-checked by a unit test) so the emitted ruleset and the software
/// [`EgressEvaluator`] can never drift: every class the evaluator denies is dropped here at the
/// network layer too. R0.1 (DELTA now-live HIGH): these are dropped EXPLICITLY on top of the default
/// `policy drop` as defence in depth, so a rule-ordering regression cannot silently open a class.
pub const ALWAYS_BLOCKED_EGRESS_CIDRS: &[&str] = &[
    "169.254.169.254/32", // CLOUD_METADATA_IP — the SSRF→cred-theft target, dropped as a /32 first.
    "169.254.0.0/16",     // LINK_LOCAL_PREFIX (169.254.) — metadata + link-local SSRF pivots.
    "10.0.0.0/8",         // "10." — RFC-1918 /8 (internal-RPC / control-plane / cross-tenant).
    "172.16.0.0/12",      // is_private_172 — RFC-1918 /12 (172.16.* .. 172.31.*).
    "192.168.0.0/16",     // "192.168." — RFC-1918 /16.
    "127.0.0.0/8",        // "127." — loopback (host-localhost services).
    "0.0.0.0/8",          // "0." — this-host / SSRF normalisation trick.
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

// ============================================================================================
// R0.1 (DELTA now-live HIGH) — the fail-closed per-tap egress firewall.
//
// SECURITY INVARIANT: no egress-capable NIC may EVER be attached to a Firecracker guest unless a real
// per-tap egress firewall ruleset has been EMITTED, APPLIED, and RECORDED in the attestation. Before
// this cluster the profile's `network_device` bool alone caused `firecracker.rs` to emit a raw,
// unfiltered `tap-myelin` NIC whenever the allowlist was non-empty — the [`EgressEvaluator`] computed
// per-host allow/deny in SOFTWARE, but nothing enforced it at the network layer, so the guest could
// reach 169.254.169.254, loopback, and cross-tenant RFC-1918 ranges over a wide-open NIC while the
// attestation ([`HardeningProfile::assert_enforced`]) falsely returned Ok. The fix makes the NIC and
// the enforced-egress ruleset ONE INDIVISIBLE THING: the NIC is gated on an [`EnforcedEgress`] record
// that can only be produced by [`EgressEnforcer::apply`]; if enforcement cannot be applied, NO NIC is
// attached and the egress-requesting job is refused with a typed error — never a silent unfiltered NIC.
// (gVisor is unaffected: it uses `--network=none` unconditionally, so it has no tap to filter.)
// ============================================================================================

/// A fail-closed egress-enforcement error (R0.1). Either variant REFUSES the job — the NIC is never
/// attached — rather than falling back to an unfiltered device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EgressEnforceError {
    /// An allowlist entry is **not an IPv4 literal** and therefore cannot be safely enforced by an IP
    /// firewall: a hostname's A-record can change (DNS rebinding) between resolution and packet time,
    /// so an `ip daddr` rule pinned to whatever the host resolved is not a sound boundary. We fail
    /// closed instead of pretending to enforce it.
    ///
    /// NAMED FUTURE FOLLOW-UP (the reason this variant exists): safely allowing hostname destinations
    /// requires a **resolving egress proxy** that re-checks the name against the allowlist on every
    /// connection (so the guest never talks to a raw IP the firewall blessed once). That proxy is NOT
    /// built here; until it exists, hostname allowlist entries make the job unenforceable → refused.
    UnenforceableHostname(String),
    /// The enforcer's apply step failed (e.g. `nft -f` returned non-zero, or `nft` is absent / the
    /// process lacks the capability). The NIC is NOT attached; the job is refused.
    ApplyFailed(String),
}

impl std::fmt::Display for EgressEnforceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EgressEnforceError::UnenforceableHostname(h) => write!(
                f,
                "egress allowlist entry `{h}` is not an IP literal — hostnames cannot be enforced by \
                 an IP firewall (DNS rebinding); a resolving egress proxy is the named follow-up. \
                 Refusing the job fail-closed rather than attaching an unfiltered NIC."
            ),
            EgressEnforceError::ApplyFailed(e) => {
                write!(f, "egress firewall ruleset could not be APPLIED ({e}) — refusing the job; no NIC attached")
            }
        }
    }
}

impl std::error::Error for EgressEnforceError {}

/// The RECORDED proof that a per-tap egress firewall ruleset was EMITTED and APPLIED to the tap device
/// (R0.1). It carries the exact ruleset text that was applied — the attestation record. Its presence
/// on a [`HardeningProfile`] / `FcMachineConfig` is the SOLE authority to attach the egress NIC: the
/// machine-config JSON emits `network-interfaces` iff an `EnforcedEgress` is present, so a NIC cannot
/// be emitted from a bare bool.
///
/// Construction is deliberately restricted: the only production mint site is [`EgressEnforcer::apply`]
/// (crate-internal constructor), so a NIC-bearing config cannot be assembled without having actually
/// applied a ruleset. Unit tests in this crate may mint one via the same crate-internal constructor to
/// drive the control flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnforcedEgress {
    /// The exact nftables ruleset text that was applied (the attestation record).
    ruleset: String,
}

impl EnforcedEgress {
    /// Mint the attestation record for an APPLIED ruleset. `pub(crate)` on purpose: the ONLY
    /// production caller is [`EgressEnforcer::apply`]'s impl (after `nft -f` succeeded); keeping the
    /// constructor crate-internal is what makes "a NIC implies an applied ruleset" a structural
    /// property rather than a convention.
    pub(crate) fn new(ruleset: String) -> EnforcedEgress {
        EnforcedEgress { ruleset }
    }

    /// The exact ruleset text that was applied (the attestation record; asserted over by tests).
    pub fn ruleset(&self) -> &str {
        &self.ruleset
    }
}

/// The apply-seam for the egress firewall (R0.1). The real production impl (in `firecracker.rs`, the
/// `no-host-exec` named-exclusion file) runs `nft -f` to install the ruleset on the host; a test double
/// lets unit tests drive the fail-closed control flow without root / `nft`. Applying returns the
/// [`EnforcedEgress`] attestation on success; on failure it returns [`EgressEnforceError::ApplyFailed`]
/// and the NIC is NOT attached.
pub trait EgressEnforcer {
    /// Apply `ruleset` to the tap device. On success, return the recorded attestation (the ruleset
    /// that is now in force). On failure, return an error — the caller MUST fail closed (no NIC).
    fn apply(&self, ruleset: &str) -> Result<EnforcedEgress, EgressEnforceError>;
}

/// Emit the deterministic nftables ruleset for the [`EGRESS_TAP_DEVICE`] egress firewall from a
/// profile's allowlist (R0.1). The text is generated deterministically so a test can assert over it.
///
/// The ruleset (a) default-DROPs (`policy drop`), (b) explicitly drops every always-blocked class
/// ([`ALWAYS_BLOCKED_EGRESS_CIDRS`] — the firewall form of what [`EgressEvaluator`] denies) as defence
/// in depth, and (c) permits ONLY allowlist destinations that are IP literals surviving the
/// always-blocked check (classified by REUSING [`EgressEvaluator`], never re-implemented).
///
/// FAIL-CLOSED enforceability boundary: an allowlist entry that is not an IPv4 literal (a hostname, or
/// a CIDR the exact-match evaluator does not model) is UNENFORCEABLE by an IP firewall (DNS rebinding),
/// so this returns [`EgressEnforceError::UnenforceableHostname`] and the job is refused — see that
/// variant for the resolving-proxy follow-up that is the reason hostnames are deferred, not enforced.
pub fn emit_egress_ruleset(profile: &HardeningProfile) -> Result<String, EgressEnforceError> {
    let policy = EgressPolicy {
        allow: profile.egress_allowlist.clone(),
    };
    let eval = EgressEvaluator::new(&policy);

    // Classify the allowlist. Fail closed on the FIRST unenforceable (non-IP-literal) entry; collect
    // the IP literals that the evaluator PERMITS (public + explicitly allowlisted). Always-blocked IP
    // literals are silently not-permitted here (the explicit drops + the always-blocked check in
    // `assert_enforced` cover them — defence in depth), never accepted.
    let mut permitted: Vec<String> = Vec::new();
    for entry in &profile.egress_allowlist {
        let host = entry.trim();
        if host.parse::<std::net::Ipv4Addr>().is_err() {
            return Err(EgressEnforceError::UnenforceableHostname(host.to_string()));
        }
        if let EgressDecision::Allow = eval.evaluate(host) {
            permitted.push(host.to_string());
        }
    }

    let mut out = String::new();
    out.push_str(
        "# R0.1 (DELTA now-live HIGH) — per-tap egress firewall for the Firecracker guest.\n\
         # Default-DROP; always-blocked classes dropped explicitly (defence in depth); ONLY IP-literal\n\
         # allowlist destinations surviving the EgressEvaluator are accepted. Hostnames are refused\n\
         # upstream (unenforceable under DNS rebinding — see EgressEnforceError::UnenforceableHostname).\n",
    );
    out.push_str("table ip myelin_egress {\n");
    out.push_str("\tchain egress {\n");
    out.push_str("\t\ttype filter hook forward priority 0; policy drop;\n");
    // (b) explicit drops of the always-blocked classes, ingress from the guest's tap.
    for cidr in ALWAYS_BLOCKED_EGRESS_CIDRS {
        out.push_str(&format!(
            "\t\tiifname \"{EGRESS_TAP_DEVICE}\" ip daddr {cidr} drop\n"
        ));
    }
    // (c) permit the enforceable IP-literal allowlist destinations.
    for ip in &permitted {
        out.push_str(&format!(
            "\t\tiifname \"{EGRESS_TAP_DEVICE}\" ip daddr {ip} accept\n"
        ));
    }
    out.push_str("\t}\n}\n");
    Ok(out)
}

/// Drive the fail-closed egress enforcement for a derived profile (R0.1): emit the ruleset, apply it
/// through the injected [`EgressEnforcer`], and return the recorded [`EnforcedEgress`] — the sole token
/// that authorises attaching the NIC. An EMPTY allowlist needs no NIC and returns `Ok(None)` (the
/// common case, unchanged). A non-empty allowlist that cannot be emitted (a hostname) or cannot be
/// applied (`nft` failed) returns `Err` — the caller MUST NOT attach a NIC.
pub fn enforce_egress(
    profile: &HardeningProfile,
    enforcer: &dyn EgressEnforcer,
) -> Result<Option<EnforcedEgress>, EgressEnforceError> {
    if profile.egress_allowlist.is_empty() {
        return Ok(None); // No egress requested ⇒ no NIC ⇒ nothing to enforce (strongest default-deny).
    }
    let ruleset = emit_egress_ruleset(profile)?;
    let enforced = enforcer.apply(&ruleset)?;
    Ok(Some(enforced))
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
    /// Whether the job REQUESTS a network device — `true` iff the allowlist is non-empty. This is a
    /// *requirement*, NOT proof of enforcement: `network_device == true` means "this job needs a
    /// filtered NIC", but the NIC may be attached ONLY once [`enforced_egress`](Self::enforced_egress)
    /// carries the applied ruleset (R0.1). An empty allowlist ⇒ `false` ⇒ no NIC at all (the strongest
    /// default-deny, egress closed at the device level).
    pub network_device: bool,
    /// The RECORDED proof that the per-tap egress firewall was emitted+applied (R0.1). `None` after
    /// [`derive`](Self::derive); set to `Some` by the launch path ONLY after [`enforce_egress`]
    /// succeeds. It is the SOLE authority to attach the NIC: [`assert_enforced`](Self::assert_enforced)
    /// rejects any profile that claims [`network_device`](Self::network_device) without this record, so
    /// a network-attached profile can never attest green without a firewall actually in force.
    pub enforced_egress: Option<EnforcedEgress>,
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
            // R0.1: derive records NO enforcement — the ruleset is emitted+applied later on the launch
            // path (fail-closed) and only then does `enforced_egress` become `Some`. A freshly-derived
            // profile that needs a NIC therefore does NOT yet pass `assert_enforced` (by design).
            enforced_egress: None,
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
        // R0.1 (DELTA now-live HIGH): a profile that claims a network device MUST carry the recorded
        // proof that the per-tap egress firewall was emitted+applied. Without it the attestation would
        // be lying — a NIC with no enforcement in force (the exact hole this cluster closes). Fail
        // closed: a network-attached profile with no `enforced_egress` record is NOT enforced-clean.
        if self.network_device && self.enforced_egress.is_none() {
            return Err(
                "network device is claimed but no enforced-egress ruleset is recorded (R0.1: a NIC \
                 may not be attached without an applied per-tap egress firewall)"
                    .into(),
            );
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
    use crate::{IdemToken, MeterTarget, RunTokenCredential};
    use crate::{ImageRef, JobKind, ResourceLimits, TrustTier, WorkspaceSpec};

    fn spec_with_egress(allow: Vec<String>) -> JobSpec {
        JobSpec::new(
            JobKind::Ci,
            ImageRef::pinned(
                "r/img@sha256:abc123def4567890abc123def4567890abc123def4567890abc123def4567890",
            )
            .unwrap(),
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
            RunTokenCredential::new("test-bearer", "j", 300).unwrap(),
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

    // --- R0.1: the fail-closed per-tap egress firewall (emit + enforce + honest attestation) ---

    #[test]
    fn always_blocked_cidrs_cover_every_classifier_prefix() {
        // The firewall CIDR list must not drift from the software classifier: every always-blocked
        // prefix/class the EgressEvaluator denies has an explicit-drop CIDR in the emitted ruleset.
        let cidrs = ALWAYS_BLOCKED_EGRESS_CIDRS.join(" ");
        for prefix in ALWAYS_BLOCKED_PRIVATE_PREFIXES {
            // Map the "10." / "192.168." / … dotted prefix to its network address (append zeros).
            let net = match *prefix {
                "10." => "10.0.0.0/8",
                "192.168." => "192.168.0.0/16",
                "127." => "127.0.0.0/8",
                "169.254." => "169.254.0.0/16",
                "0." => "0.0.0.0/8",
                other => panic!("unmapped always-blocked prefix {other} — update the CIDR list"),
            };
            assert!(
                cidrs.contains(net),
                "always-blocked prefix {prefix} ({net}) is missing from the firewall CIDR list"
            );
        }
        assert!(
            cidrs.contains("172.16.0.0/12"),
            "the 172.16/12 range must be dropped"
        );
        assert!(
            cidrs.contains("169.254.169.254/32"),
            "the metadata IP must be dropped as an explicit /32"
        );
    }

    #[test]
    fn emit_ruleset_default_drops_and_never_permits_a_blocked_class() {
        // A job requesting egress to a public IP: the ruleset default-drops, drops every blocked
        // class, and permits ONLY the public IP — never 169.254.169.254 / 10.x / 172.16-31 /
        // 192.168 / 127 / 0.x.
        let p = HardeningProfile::derive(&spec_with_egress(vec!["93.184.216.34".into()]));
        let rs = emit_egress_ruleset(&p).expect("public IP allowlist is enforceable");
        assert!(rs.contains("policy drop;"), "the chain MUST default-DROP");
        assert!(
            rs.contains("ip daddr 93.184.216.34 accept"),
            "the public IP is the ONLY accepted destination"
        );
        // None of the always-blocked classes may ever be ACCEPTED (they appear only as drops).
        for blocked in [
            "169.254.169.254/32",
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "127.0.0.0/8",
            "0.0.0.0/8",
        ] {
            assert!(
                !rs.contains(&format!("ip daddr {blocked} accept")),
                "{blocked} must NEVER be accepted — it is always-blocked"
            );
            assert!(
                rs.contains(&format!("ip daddr {blocked} drop")),
                "{blocked} must be explicitly dropped (defence in depth)"
            );
        }
        // Deterministic: emitting twice yields identical text.
        assert_eq!(rs, emit_egress_ruleset(&p).unwrap());
    }

    #[test]
    fn emit_ruleset_refuses_a_hostname_fail_closed() {
        // A hostname allowlist entry is UNENFORCEABLE (DNS rebinding) → fail closed with the typed err.
        let p = HardeningProfile::derive(&spec_with_egress(vec!["registry.example.com".into()]));
        assert_eq!(
            emit_egress_ruleset(&p),
            Err(EgressEnforceError::UnenforceableHostname(
                "registry.example.com".into()
            ))
        );
    }

    #[test]
    fn emit_ruleset_never_accepts_an_always_blocked_ip_literal() {
        // An IP literal that is always-blocked (10.0.0.1) is a valid literal but must NOT be accepted.
        let p = HardeningProfile::derive(&spec_with_egress(vec!["10.0.0.1".into()]));
        let rs =
            emit_egress_ruleset(&p).expect("an IP literal is enforceable (just not permitted)");
        assert!(!rs.contains("ip daddr 10.0.0.1 accept"));
    }

    /// A test enforcer that records the ruleset it was handed and can be set to fail.
    struct TestEnforcer {
        fail: bool,
        seen: std::sync::Mutex<Option<String>>,
    }
    impl EgressEnforcer for TestEnforcer {
        fn apply(&self, ruleset: &str) -> Result<EnforcedEgress, EgressEnforceError> {
            *self.seen.lock().unwrap() = Some(ruleset.to_string());
            if self.fail {
                Err(EgressEnforceError::ApplyFailed(
                    "injected nft failure".into(),
                ))
            } else {
                Ok(EnforcedEgress::new(ruleset.to_string()))
            }
        }
    }

    #[test]
    fn enforce_egress_empty_allowlist_needs_no_nic() {
        let p = HardeningProfile::derive(&spec_with_egress(vec![]));
        let enf = TestEnforcer {
            fail: false,
            seen: std::sync::Mutex::new(None),
        };
        assert_eq!(enforce_egress(&p, &enf).unwrap(), None);
        assert!(
            enf.seen.lock().unwrap().is_none(),
            "no ruleset applied when no NIC is needed"
        );
    }

    #[test]
    fn enforce_egress_records_the_applied_ruleset() {
        let p = HardeningProfile::derive(&spec_with_egress(vec!["93.184.216.34".into()]));
        let enf = TestEnforcer {
            fail: false,
            seen: std::sync::Mutex::new(None),
        };
        let rec = enforce_egress(&p, &enf)
            .unwrap()
            .expect("a NIC needs a recorded ruleset");
        assert!(rec.ruleset().contains("policy drop;"));
        assert_eq!(rec.ruleset(), enf.seen.lock().unwrap().as_deref().unwrap());
    }

    #[test]
    fn enforce_egress_fails_closed_when_apply_fails() {
        let p = HardeningProfile::derive(&spec_with_egress(vec!["93.184.216.34".into()]));
        let enf = TestEnforcer {
            fail: true,
            seen: std::sync::Mutex::new(None),
        };
        assert_eq!(
            enforce_egress(&p, &enf),
            Err(EgressEnforceError::ApplyFailed(
                "injected nft failure".into()
            ))
        );
    }

    #[test]
    fn assert_enforced_rejects_a_network_device_without_a_recorded_ruleset() {
        // A hand-constructed profile claiming a NIC but carrying no enforced-egress record is a LIE.
        let mut p = HardeningProfile::derive(&spec_with_egress(vec!["93.184.216.34".into()]));
        assert!(p.network_device);
        assert!(p.enforced_egress.is_none());
        assert!(
            p.assert_enforced().is_err(),
            "a NIC without an applied ruleset must NOT attest green (R0.1)"
        );
        // Once the enforcement record is present, the same profile attests green.
        p.enforced_egress = Some(EnforcedEgress::new(emit_egress_ruleset(&p).unwrap()));
        assert!(p.assert_enforced().is_ok());
    }
}

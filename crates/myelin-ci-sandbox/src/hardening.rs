use crate::{EgressPolicy, JobSpec};
use serde::{Deserialize, Serialize};

pub const CLOUD_METADATA_IP: &str = "169.254.169.254";

pub const LINK_LOCAL_PREFIX: &str = "169.254.";

pub const ALWAYS_BLOCKED_PRIVATE_PREFIXES: &[&str] = &[
    "10.",
    "192.168.",
    "127.",
    "169.254.",
    "0.",
];

pub const EGRESS_TAP_DEVICE: &str = "tap-myelin";

pub const ALWAYS_BLOCKED_EGRESS_CIDRS: &[&str] = &[
    "169.254.169.254/32",
    "169.254.0.0/16",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "127.0.0.0/8",
    "0.0.0.0/8",
];

fn is_private_172(host: &str) -> bool {
    let Some(rest) = host.strip_prefix("172.") else {
        return false;
    };
    let Some((octet, _)) = rest.split_once('.') else {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EgressDecision {
    Allow,
    Deny(DenyReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenyReason {
    CloudMetadata,
    InternalOrCrossTenant,
    DefaultDeny,
}

#[derive(Clone, Debug)]
pub struct EgressEvaluator<'a> {
    policy: &'a EgressPolicy,
}

impl<'a> EgressEvaluator<'a> {
    pub fn new(policy: &'a EgressPolicy) -> EgressEvaluator<'a> {
        EgressEvaluator { policy }
    }

    pub fn evaluate(&self, host: &str) -> EgressDecision {
        let host = host.trim();

        if host == CLOUD_METADATA_IP {
            return EgressDecision::Deny(DenyReason::CloudMetadata);
        }
        if ALWAYS_BLOCKED_PRIVATE_PREFIXES
            .iter()
            .any(|p| host.starts_with(p))
            || is_private_172(host)
        {
            if host.starts_with(LINK_LOCAL_PREFIX) {
                return EgressDecision::Deny(DenyReason::InternalOrCrossTenant);
            }
            return EgressDecision::Deny(DenyReason::InternalOrCrossTenant);
        }
        if self.policy.allow.iter().any(|entry| entry.trim() == host) {
            return EgressDecision::Allow;
        }
        EgressDecision::Deny(DenyReason::DefaultDeny)
    }

    pub fn is_allowed(&self, host: &str) -> bool {
        matches!(self.evaluate(host), EgressDecision::Allow)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EgressEnforceError {
    UnenforceableHostname(String),
    ApplyFailed(String),
}

impl std::fmt::Display for EgressEnforceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EgressEnforceError::UnenforceableHostname(h) => write!(
                f,
                "egress allowlist entry `{h}` is not an IP literal - hostnames cannot be enforced by \
                 an IP firewall (DNS rebinding); a resolving egress proxy is the named follow-up. \
                 Refusing the job fail-closed rather than attaching an unfiltered NIC."
            ),
            EgressEnforceError::ApplyFailed(e) => {
                write!(f, "egress firewall ruleset could not be APPLIED ({e}) - refusing the job; no NIC attached")
            }
        }
    }
}

impl std::error::Error for EgressEnforceError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnforcedEgress {
    ruleset: String,
}

impl EnforcedEgress {
    pub(crate) fn new(ruleset: String) -> EnforcedEgress {
        EnforcedEgress { ruleset }
    }

    pub fn ruleset(&self) -> &str {
        &self.ruleset
    }
}

pub trait EgressEnforcer {
    fn apply(&self, ruleset: &str) -> Result<EnforcedEgress, EgressEnforceError>;
}

pub fn emit_egress_ruleset(profile: &HardeningProfile) -> Result<String, EgressEnforceError> {
    let policy = EgressPolicy {
        allow: profile.egress_allowlist.clone(),
    };
    let eval = EgressEvaluator::new(&policy);

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
    out.push_str("table ip myelin_egress {\n");
    out.push_str("\tchain egress {\n");
    out.push_str("\t\ttype filter hook forward priority 0; policy drop;\n");
    for cidr in ALWAYS_BLOCKED_EGRESS_CIDRS {
        out.push_str(&format!(
            "\t\tiifname \"{EGRESS_TAP_DEVICE}\" ip daddr {cidr} drop\n"
        ));
    }
    for ip in &permitted {
        out.push_str(&format!(
            "\t\tiifname \"{EGRESS_TAP_DEVICE}\" ip daddr {ip} accept\n"
        ));
    }
    out.push_str("\t}\n}\n");
    Ok(out)
}

pub fn enforce_egress(
    profile: &HardeningProfile,
    enforcer: &dyn EgressEnforcer,
) -> Result<Option<EnforcedEgress>, EgressEnforceError> {
    if profile.egress_allowlist.is_empty() {
        return Ok(None);
    }
    let ruleset = emit_egress_ruleset(profile)?;
    let enforced = enforcer.apply(&ruleset)?;
    Ok(Some(enforced))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardeningProfile {
    pub egress_default_deny: bool,
    pub egress_allowlist: Vec<String>,
    pub network_device: bool,
    pub enforced_egress: Option<EnforcedEgress>,
    pub read_only_root: bool,
    pub drop_all_caps: bool,
    pub no_new_privileges: bool,
    pub seccomp: bool,
    pub pids_max: u32,
    pub zero_swap: bool,
    pub scratch_quota_bytes: u64,
    pub ephemeral_one_job: bool,
}

impl HardeningProfile {
    pub fn derive(spec: &JobSpec) -> HardeningProfile {
        Self::for_execution(&spec.limits, &spec.egress)
    }

    pub(crate) fn for_execution(
        limits: &crate::ResourceLimits,
        egress: &crate::EgressPolicy,
    ) -> HardeningProfile {
        let allowlist = egress.allow.clone();
        let needs_nic = !allowlist.is_empty();
        HardeningProfile {
            egress_default_deny: true,
            egress_allowlist: allowlist,
            network_device: needs_nic,
            enforced_egress: None,
            read_only_root: true,
            drop_all_caps: true,
            no_new_privileges: true,
            seccomp: true,
            pids_max: limits.pids_max,
            zero_swap: true,
            scratch_quota_bytes: limits.tmpfs_bytes,
            ephemeral_one_job: true,
        }
    }

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
        if self.network_device && self.enforced_egress.is_none() {
            return Err(
                "network device is claimed but no enforced-egress ruleset is recorded (R0.1: a NIC \
                 may not be attached without an applied per-tap egress firewall)"
                    .into(),
            );
        }
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
                tmpfs_bytes: 1 << 30,
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

    #[test]
    fn cloud_metadata_is_always_denied_even_if_allowlisted() {
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
        let internal = [
            "10.0.0.5",
            "10.255.255.255",
            "192.168.1.1",
            "172.16.0.1",
            "172.31.255.254",
            "127.0.0.1",
            "169.254.10.20",
            "0.0.0.0",
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
        let policy = EgressPolicy {
            allow: vec!["172.15.0.1".into(), "172.32.0.1".into()],
        };
        let eval = EgressEvaluator::new(&policy);
        assert_eq!(eval.evaluate("172.15.0.1"), EgressDecision::Allow);
        assert_eq!(eval.evaluate("172.32.0.1"), EgressDecision::Allow);
        assert_eq!(
            eval.evaluate("172.20.0.1"),
            EgressDecision::Deny(DenyReason::InternalOrCrossTenant)
        );
    }

    #[test]
    fn default_deny_is_the_baseline_for_unlisted_public_destinations() {
        let policy = EgressPolicy::deny_all();
        let eval = EgressEvaluator::new(&policy);
        assert_eq!(
            eval.evaluate("93.184.216.34"),
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
        assert!(!eval.is_allowed("evil.example.net"));
        assert!(!eval.is_allowed(CLOUD_METADATA_IP));
        assert!(!eval.is_allowed("10.0.0.1"));
    }

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
        let mut p = HardeningProfile::derive(&spec_with_egress(vec![]));
        p.egress_allowlist = vec![CLOUD_METADATA_IP.into()];
        assert!(p.assert_enforced().is_err());
    }

    #[test]
    fn always_blocked_cidrs_cover_every_classifier_prefix() {
        let cidrs = ALWAYS_BLOCKED_EGRESS_CIDRS.join(" ");
        for prefix in ALWAYS_BLOCKED_PRIVATE_PREFIXES {
            let net = match *prefix {
                "10." => "10.0.0.0/8",
                "192.168." => "192.168.0.0/16",
                "127." => "127.0.0.0/8",
                "169.254." => "169.254.0.0/16",
                "0." => "0.0.0.0/8",
                other => panic!("unmapped always-blocked prefix {other} - update the CIDR list"),
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
        let p = HardeningProfile::derive(&spec_with_egress(vec!["93.184.216.34".into()]));
        let rs = emit_egress_ruleset(&p).expect("public IP allowlist is enforceable");
        assert!(rs.contains("policy drop;"), "the chain MUST default-DROP");
        assert!(
            rs.contains("ip daddr 93.184.216.34 accept"),
            "the public IP is the ONLY accepted destination"
        );
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
                "{blocked} must NEVER be accepted - it is always-blocked"
            );
            assert!(
                rs.contains(&format!("ip daddr {blocked} drop")),
                "{blocked} must be explicitly dropped (defence in depth)"
            );
        }
        assert_eq!(rs, emit_egress_ruleset(&p).unwrap());
    }

    #[test]
    fn emit_ruleset_refuses_a_hostname_fail_closed() {
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
        let p = HardeningProfile::derive(&spec_with_egress(vec!["10.0.0.1".into()]));
        let rs =
            emit_egress_ruleset(&p).expect("an IP literal is enforceable (just not permitted)");
        assert!(!rs.contains("ip daddr 10.0.0.1 accept"));
    }

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
        let mut p = HardeningProfile::derive(&spec_with_egress(vec!["93.184.216.34".into()]));
        assert!(p.network_device);
        assert!(p.enforced_egress.is_none());
        assert!(
            p.assert_enforced().is_err(),
            "a NIC without an applied ruleset must NOT attest green (R0.1)"
        );
        p.enforced_egress = Some(EnforcedEgress::new(emit_egress_ruleset(&p).unwrap()));
        assert!(p.assert_enforced().is_ok());
    }
}

use myelin_tenancy::Region;

use crate::registries::{is_eea_region, TransferGate};

pub const OUTBOUND_MIRROR_PII_TRANSFERS_BLOCKED: (&str, &str) =
    ("gdpr.outbound_mirror_pii_transfers_blocked", "count");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutboundConfigKind {
    PushMirror,
    CdnClone,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboundConfig {
    pub kind: OutboundConfigKind,
    pub target_region: Region,
    pub pii_bearing: bool,
}

impl OutboundConfig {
    pub fn push_mirror(target_region: Region) -> OutboundConfig {
        OutboundConfig {
            kind: OutboundConfigKind::PushMirror,
            target_region,
            pii_bearing: true,
        }
    }

    pub fn cdn_clone(target_region: Region) -> OutboundConfig {
        OutboundConfig {
            kind: OutboundConfigKind::CdnClone,
            target_region,
            pii_bearing: true,
        }
    }

    pub fn pii_free(mut self) -> OutboundConfig {
        self.pii_bearing = false;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutboundDecision {
    Allow { reason: OutboundAllowReason },
    Deny { reason: OutboundDenyReason },
}

impl OutboundDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, OutboundDecision::Allow { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutboundAllowReason {
    WithinEuCdnAcceleration,
    WithinEuPushMirror,
    LawfulExtraEuTransfer,
    NonPiiBearing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutboundDenyReason {
    ExtraEuReplicationDeniedByDefault { target_region: Region },
    ExtraEuCdnEdgeDenied { target_region: Region },
}

#[derive(Default)]
pub struct OutboundMirrorGate {
    extra_eu_pii_transfers_blocked: u64,
}

impl OutboundMirrorGate {
    pub fn new() -> OutboundMirrorGate {
        OutboundMirrorGate::default()
    }

    pub fn decide(
        &mut self,
        config: &OutboundConfig,
        transfer_policy: &TransferGate,
    ) -> OutboundDecision {
        if !config.pii_bearing {
            return OutboundDecision::Allow {
                reason: OutboundAllowReason::NonPiiBearing,
            };
        }

        match config.kind {
            OutboundConfigKind::CdnClone => {
                if is_eea_region(&config.target_region) {
                    OutboundDecision::Allow {
                        reason: OutboundAllowReason::WithinEuCdnAcceleration,
                    }
                } else {
                    self.extra_eu_pii_transfers_blocked += 1;
                    OutboundDecision::Deny {
                        reason: OutboundDenyReason::ExtraEuCdnEdgeDenied {
                            target_region: config.target_region.clone(),
                        },
                    }
                }
            }
            OutboundConfigKind::PushMirror => {
                if is_eea_region(&config.target_region) {
                    OutboundDecision::Allow {
                        reason: OutboundAllowReason::WithinEuPushMirror,
                    }
                } else if transfer_policy
                    .transfer_allowed(&config.target_region)
                    .is_allowed()
                {
                    OutboundDecision::Allow {
                        reason: OutboundAllowReason::LawfulExtraEuTransfer,
                    }
                } else {
                    self.extra_eu_pii_transfers_blocked += 1;
                    OutboundDecision::Deny {
                        reason: OutboundDenyReason::ExtraEuReplicationDeniedByDefault {
                            target_region: config.target_region.clone(),
                        },
                    }
                }
            }
        }
    }

    pub fn extra_eu_pii_transfers_blocked(&self) -> u64 {
        self.extra_eu_pii_transfers_blocked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_eu_push_mirror_denied_within_eu_cdn_clone_allowed() {
        let policy = TransferGate::new();
        let mut gate = OutboundMirrorGate::new();

        let extra_eu_mirror = OutboundConfig::push_mirror(Region::new("us-east"));
        let denied = gate.decide(&extra_eu_mirror, &policy);
        assert_eq!(
            denied,
            OutboundDecision::Deny {
                reason: OutboundDenyReason::ExtraEuReplicationDeniedByDefault {
                    target_region: Region::new("us-east"),
                },
            },
            "a PII-bearing extra-EU push-mirror is denied by default (§5.3)"
        );
        assert!(
            !denied.is_allowed(),
            "the caller must NOT replicate on a Deny"
        );

        let within_eu_clone = OutboundConfig::cdn_clone(Region::new("fr-par"));
        let allowed = gate.decide(&within_eu_clone, &policy);
        assert_eq!(
            allowed,
            OutboundDecision::Allow {
                reason: OutboundAllowReason::WithinEuCdnAcceleration,
            },
            "a within-EU CDN clone is permitted (§5.3 acceleration)"
        );
        assert!(allowed.is_allowed());

        assert_eq!(
            gate.extra_eu_pii_transfers_blocked(),
            1,
            "0 default extra-EU PII transfers slip through (the one mirror was blocked)"
        );
    }

    #[test]
    fn extra_eu_cdn_clone_is_a_disguised_replication_denied() {
        let policy = TransferGate::new();
        let mut gate = OutboundMirrorGate::new();

        let extra_eu_clone = OutboundConfig::cdn_clone(Region::new("us-east"));
        let decision = gate.decide(&extra_eu_clone, &policy);
        assert_eq!(
            decision,
            OutboundDecision::Deny {
                reason: OutboundDenyReason::ExtraEuCdnEdgeDenied {
                    target_region: Region::new("us-east"),
                },
            },
            "a CDN clone pointing extra-EU is a disguised replication - denied (no extra-EU edge serves PII)"
        );
        assert_eq!(
            gate.extra_eu_pii_transfers_blocked(),
            1,
            "the disguised extra-EU clone is counted as a blocked PII transfer"
        );
    }

    #[test]
    fn within_eu_cross_region_push_mirror_allowed() {
        let policy = TransferGate::new();
        let mut gate = OutboundMirrorGate::new();

        let within_eu_mirror = OutboundConfig::push_mirror(Region::new("nl-ams"));
        let decision = gate.decide(&within_eu_mirror, &policy);
        assert_eq!(
            decision,
            OutboundDecision::Allow {
                reason: OutboundAllowReason::WithinEuPushMirror,
            },
            "a within-EU push-mirror crosses no sovereignty boundary (§5.3)"
        );
        assert_eq!(
            gate.extra_eu_pii_transfers_blocked(),
            0,
            "a within-EU push-mirror is not a blocked extra-EU transfer"
        );
    }

    #[test]
    fn extra_eu_push_mirror_allowed_only_with_recorded_mechanism() {
        let policy = TransferGate::new();
        let mut gate = OutboundMirrorGate::new();
        let mirror = OutboundConfig::push_mirror(Region::new("us-east"));

        assert!(
            !gate.decide(&mirror, &policy).is_allowed(),
            "denied by default before a lawful basis is recorded"
        );
        assert_eq!(gate.extra_eu_pii_transfers_blocked(), 1);

        policy.record_transfer_mechanism(Region::new("us-east"));
        let decision = gate.decide(&mirror, &policy);
        assert_eq!(
            decision,
            OutboundDecision::Allow {
                reason: OutboundAllowReason::LawfulExtraEuTransfer,
            },
            "an extra-EU push-mirror WITH a recorded transfer mechanism is permitted (the lawful path)"
        );
        assert_eq!(
            gate.extra_eu_pii_transfers_blocked(),
            1,
            "the lawful-transfer allow did not bump the blocked count"
        );
    }

    #[test]
    fn pii_free_config_is_allowed_even_extra_eu() {
        let policy = TransferGate::new();
        let mut gate = OutboundMirrorGate::new();

        let pii_free_mirror = OutboundConfig::push_mirror(Region::new("us-east")).pii_free();
        let decision = gate.decide(&pii_free_mirror, &policy);
        assert_eq!(
            decision,
            OutboundDecision::Allow {
                reason: OutboundAllowReason::NonPiiBearing,
            },
            "a PII-free config is not a transfer of personal data - allowed even extra-EU"
        );
        assert_eq!(
            gate.extra_eu_pii_transfers_blocked(),
            0,
            "a PII-free config is not a blocked PII transfer"
        );
    }

    #[test]
    fn the_default_config_is_pii_bearing_fail_closed() {
        assert!(
            OutboundConfig::push_mirror(Region::new("us-east")).pii_bearing,
            "a push-mirror defaults to PII-bearing (fail-closed)"
        );
        assert!(
            OutboundConfig::cdn_clone(Region::new("fr-par")).pii_bearing,
            "a CDN clone defaults to PII-bearing (fail-closed)"
        );
        assert!(
            !OutboundConfig::push_mirror(Region::new("us-east"))
                .pii_free()
                .pii_bearing,
            "pii_free() flips it off"
        );
    }

    #[test]
    fn blocked_counter_is_a_running_total() {
        let policy = TransferGate::new();
        let mut gate = OutboundMirrorGate::new();

        gate.decide(
            &OutboundConfig::push_mirror(Region::new("us-east")),
            &policy,
        );
        gate.decide(&OutboundConfig::cdn_clone(Region::new("ap-tokyo")), &policy);
        gate.decide(&OutboundConfig::cdn_clone(Region::new("fr-par")), &policy);
        gate.decide(&OutboundConfig::push_mirror(Region::new("nl-ams")), &policy);

        assert_eq!(
            gate.extra_eu_pii_transfers_blocked(),
            2,
            "exactly the two extra-EU PII replications were blocked (the running total)"
        );
    }

    #[test]
    fn telemetry_signal_name_and_unit_are_anchored() {
        assert_eq!(
            OUTBOUND_MIRROR_PII_TRANSFERS_BLOCKED.0,
            "gdpr.outbound_mirror_pii_transfers_blocked"
        );
        assert_eq!(OUTBOUND_MIRROR_PII_TRANSFERS_BLOCKED.1, "count");
    }
}

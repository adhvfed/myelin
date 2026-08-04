use myelin_gdpr_service::{
    OutboundAllowReason, OutboundConfig, OutboundDecision, OutboundDenyReason, OutboundMirrorGate,
    TransferGate,
};
use myelin_tenancy::Region;

struct OutboundReplicationFeature {
    replications_made: u64,
    extra_eu_replications_made: u64,
}

impl OutboundReplicationFeature {
    fn new() -> OutboundReplicationFeature {
        OutboundReplicationFeature {
            replications_made: 0,
            extra_eu_replications_made: 0,
        }
    }

    fn try_replicate(
        &mut self,
        gate: &mut OutboundMirrorGate,
        policy: &TransferGate,
        config: &OutboundConfig,
        extra_eu: bool,
    ) -> OutboundDecision {
        let decision = gate.decide(config, policy);
        if decision.is_allowed() {
            self.replications_made += 1;
            if extra_eu {
                self.extra_eu_replications_made += 1;
            }
        }
        decision
    }
}

#[test]
fn ga_11_outbound_push_mirror_residency_gate_denies_extra_eu_admits_within_eu() {
    let policy = TransferGate::new();
    let mut gate = OutboundMirrorGate::new();
    let mut feature = OutboundReplicationFeature::new();

    let extra_eu_mirror = OutboundConfig::push_mirror(Region::new("us-east"));
    let d1 = feature.try_replicate(
        &mut gate,
        &policy,
        &extra_eu_mirror,
         true,
    );
    assert!(
        matches!(
            d1,
            OutboundDecision::Deny {
                reason: OutboundDenyReason::ExtraEuReplicationDeniedByDefault { .. }
            }
        ),
        "a PII-bearing extra-EU push-mirror is denied by default (§5.3)"
    );

    let within_eu_clone = OutboundConfig::cdn_clone(Region::new("fr-par"));
    let d2 = feature.try_replicate(
        &mut gate,
        &policy,
        &within_eu_clone,
         false,
    );
    assert_eq!(
        d2,
        OutboundDecision::Allow {
            reason: OutboundAllowReason::WithinEuCdnAcceleration
        },
        "a within-EU CDN clone is permitted (§5.3 acceleration)"
    );

    let disguised = OutboundConfig::cdn_clone(Region::new("ap-tokyo"));
    let d3 = feature.try_replicate(&mut gate, &policy, &disguised,  true);
    assert!(
        matches!(
            d3,
            OutboundDecision::Deny {
                reason: OutboundDenyReason::ExtraEuCdnEdgeDenied { .. }
            }
        ),
        "a CDN clone pointing extra-EU is a disguised replication - denied (§5.3)"
    );

    let within_eu_mirror = OutboundConfig::push_mirror(Region::new("nl-ams"));
    let d4 = feature.try_replicate(
        &mut gate,
        &policy,
        &within_eu_mirror,
         false,
    );
    assert_eq!(
        d4,
        OutboundDecision::Allow {
            reason: OutboundAllowReason::WithinEuPushMirror
        },
        "a within-EU push-mirror crosses no sovereignty boundary (§5.3)"
    );

    assert_eq!(
        feature.extra_eu_replications_made, 0,
        "0 PII-bearing extra-EU replications were made (the feature honoured every deny)"
    );
    assert_eq!(
        gate.extra_eu_pii_transfers_blocked(),
        2,
        "exactly the two extra-EU PII crossings were blocked (the push-mirror + the disguised clone)"
    );
    assert_eq!(
        feature.replications_made, 2,
        "only the two within-EU accelerations were distributed"
    );

    policy.record_transfer_mechanism(Region::new("us-east"));
    let d5 = gate.decide(&extra_eu_mirror, &policy);
    assert_eq!(
        d5,
        OutboundDecision::Allow {
            reason: OutboundAllowReason::LawfulExtraEuTransfer
        },
        "recording a transfer mechanism flips the extra-EU mirror to allowed (the lawful path)"
    );
    assert_eq!(
        gate.extra_eu_pii_transfers_blocked(),
        2,
        "the lawful-transfer allow did not bump the blocked count - still the two by-default denies"
    );

    println!(
        "[P-452 GA-11 GREEN 2026-06-24] outbound push-mirror residency gate: a PII-bearing extra-EU \
         push-mirror (us-east) DENIED by default + a disguised extra-EU CDN clone (ap-tokyo) DENIED \
         (extra_eu_pii_transfers_blocked=2); a within-EU CDN clone (fr-par) + a within-EU push-mirror \
         (nl-ams) ALLOWED (within-EU acceleration). 0 default extra-EU PII transfers slipped through \
         (extra_eu_replications_made=0). Recording a [OPEN - LEGAL] transfer mechanism flipped the \
         SAME extra-EU mirror to ALLOWED (the gate consults the policy, not a blanket block). RESIDUAL \
         (named, not pretended-solved, §1/§5.3): an independent off-platform clone a third party \
         already holds is outside the gate's reach - the gate prevents NEW extra-EU PII replication."
    );
}

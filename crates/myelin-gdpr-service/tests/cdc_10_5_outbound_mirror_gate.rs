use myelin_gdpr_service::{
    OutboundConfig, OutboundConfigKind, OutboundDecision, OutboundMirrorGate, TransferGate,
};
use myelin_tenancy::Region;

struct ControlPlaneEnforcementConsumer {
    tenant_region: Region,
    pushes_enforced: u64,
}

impl ControlPlaneEnforcementConsumer {
    fn for_tenant_in(region: Region) -> ControlPlaneEnforcementConsumer {
        ControlPlaneEnforcementConsumer {
            tenant_region: region,
            pushes_enforced: 0,
        }
    }

    fn enforce(
        &mut self,
        gate: &mut OutboundMirrorGate,
        policy: &TransferGate,
        config: &OutboundConfig,
    ) -> OutboundDecision {
        let decision = gate.decide(config, policy);
        if decision.is_allowed() {
            self.pushes_enforced += 1;
        }
        decision
    }
}

#[test]
fn cdc_10_5_control_plane_enforcement_reads_the_outbound_mirror_policy() {
    let policy = TransferGate::new();
    let mut gate = OutboundMirrorGate::new();
    let mut consumer = ControlPlaneEnforcementConsumer::for_tenant_in(Region::new("fr-par"));

    let extra_eu_mirror = OutboundConfig::push_mirror(Region::new("us-east"));
    let denied = consumer.enforce(&mut gate, &policy, &extra_eu_mirror);
    assert!(
        !denied.is_allowed(),
        "the §5.3 policy denies a PII-bearing extra-EU push-mirror by default → enforcement refuses"
    );
    assert_eq!(
        consumer.pushes_enforced, 0,
        "the control-plane enforcement gate did NOT push (honours the policy deny)"
    );

    let within_eu_clone = OutboundConfig::cdn_clone(consumer.tenant_region.clone());
    assert!(
        consumer
            .enforce(&mut gate, &policy, &within_eu_clone)
            .is_allowed(),
        "a within-EU CDN clone is permitted by the policy (§5.3 acceleration)"
    );
    assert_eq!(
        consumer.pushes_enforced, 1,
        "the within-EU clone was enforced"
    );

    policy.record_transfer_mechanism(Region::new("us-east"));
    let now_allowed = consumer.enforce(&mut gate, &policy, &extra_eu_mirror);
    assert!(
        now_allowed.is_allowed(),
        "an extra-EU push-mirror WITH a recorded mechanism is permitted (the lawful path)"
    );
    assert_eq!(
        consumer.pushes_enforced, 2,
        "the now-lawful transfer was enforced"
    );

    assert_eq!(
        gate.extra_eu_pii_transfers_blocked(),
        1,
        "0 default extra-EU PII transfers slipped through (the one mirror was blocked before recording)"
    );
}

#[test]
fn cdc_10_5_extra_eu_cdn_clone_is_denied_by_the_policy() {
    let policy = TransferGate::new();
    let mut gate = OutboundMirrorGate::new();
    let mut consumer = ControlPlaneEnforcementConsumer::for_tenant_in(Region::new("fr-par"));

    let disguised = OutboundConfig {
        kind: OutboundConfigKind::CdnClone,
        target_region: Region::new("ap-tokyo"),
        pii_bearing: true,
    };
    let decision = consumer.enforce(&mut gate, &policy, &disguised);
    assert!(
        matches!(decision, OutboundDecision::Deny { .. }),
        "an extra-EU CDN clone is a disguised replication - the policy denies it (§5.3)"
    );
    assert_eq!(
        consumer.pushes_enforced, 0,
        "the enforcement gate refuses the disguised extra-EU clone"
    );
    assert_eq!(gate.extra_eu_pii_transfers_blocked(), 1);
}

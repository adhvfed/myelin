//! # CDC 10.5 (the outbound-mirror gate POLICY leg) — the GA-11 `OutboundMirrorGate` ⇄ the
//! control-plane residency-enforcement consumer (P-GA-36 → P-452)
//!
//! **Contract:** index row 10.5 (the outbound-mirror gate — *"`transfer_allowed` (deny extra-EU by
//! default); + gates the outbound Git push-mirror (NEW, §5.3)"*, gdpr §5.3). This is the
//! consumer-driven contract test the coverage scanner (P-S21) reads both halves of, for the
//! outbound-mirror POLICY leg of 10.5 (the consent/sub-processor/transfer legs are P-GA-23,
//! `cdc_10_5_consent_transfer_gate.rs`; the retention leg is P-GA-22; the control-plane ENFORCEMENT
//! gate's own CDC is `myelin-control-plane/tests/cdc_10_5_mirror_gate.rs` — that one wires the REAL
//! `TransferGate` as the lawful-transfer port. THIS one proves the §5.3 within-EU-acceleration-vs-
//! extra-EU-replication policy the GA-11 gate ships is the SAME decision the control-plane enforcement
//! reads):
//!
//! - **provider** = the **GA-11 outbound-mirror gate** ([`OutboundMirrorGate`]) over the EXISTING
//!   `transfer_allowed` policy ([`TransferGate`]) — a within-EU CDN clone allowed; a within-EU
//!   push-mirror allowed; a PII-bearing extra-EU push-mirror denied by default; an extra-EU "CDN
//!   clone" denied (disguised replication); an extra-EU push-mirror allowed only with a recorded
//!   mechanism.
//! - **consumer** = the **control-plane residency-enforcement gate** (`myelin_control_plane::
//!   MirrorGate::mirror_allowed`, §12.x / §7.4) — modelled here by [`ControlPlaneEnforcementConsumer`]
//!   (the §5.3 ownership split: GDPR owns the POLICY this gate reads, Tenancy enforces). It resolves
//!   the tenant's region of record, classifies the outbound config, asks the GA-11 policy, and
//!   HONOURS the deny (it does NOT enforce a push the policy refuses).
//!
//! **Why the consumer is modelled in-test (documented deviation, EI-01 §1):** `myelin-control-plane`
//! is NOT a dependency of this service crate (and must not be — a reverse dev-edge would cross the
//! §2.9 service tier the wrong way; the production-DAG enforcement edge runs control-plane → GDPR via
//! the `TransferPolicy` port, proven by the control-plane's OWN `cdc_10_5_mirror_gate.rs`). The
//! consumer here is exercised by a faithful [`ControlPlaneEnforcementConsumer`] using ONLY the public
//! GA-11 policy surface (`decide` + `is_allowed`) the real enforcement gate consults through the
//! port. If the policy drifts (the gate stops denying extra-EU replication by default, or stops
//! distinguishing a within-EU CDN acceleration from an extra-EU one), this stops passing — that is
//! the contract.

use myelin_gdpr_service::{
    OutboundConfig, OutboundConfigKind, OutboundDecision, OutboundMirrorGate, TransferGate,
};
use myelin_tenancy::Region;

/// The control-plane residency-enforcement consumer (the §5.3 / §12.x enforcement half, modelled). It
/// has the tenant's region of record (the residency anchor); for each outbound config it asks the
/// GA-11 POLICY and HONOURS the verdict — it ENFORCES a push (bumps `pushes_enforced`) ONLY on an
/// Allow; on a Deny it refuses (0 unauthorised cross-residency pushes reach the wire).
struct ControlPlaneEnforcementConsumer {
    /// The tenant's residency region of record (the boundary's near side).
    tenant_region: Region,
    /// The pushes the enforcement gate actually let through to the wire.
    pushes_enforced: u64,
}

impl ControlPlaneEnforcementConsumer {
    fn for_tenant_in(region: Region) -> ControlPlaneEnforcementConsumer {
        ControlPlaneEnforcementConsumer {
            tenant_region: region,
            pushes_enforced: 0,
        }
    }

    /// Enforce an outbound replication for `config` by consulting the GA-11 policy gate. Honours the
    /// verdict: enforces the push ONLY on an Allow. Returns the policy decision (for assertions).
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

/// **THE CDC PAIR (dated green artifact): the GA-11 outbound-mirror policy ⇄ the control-plane
/// enforcement consumer.** Proves the enforcement gate reads the §5.3 policy and honours the
/// deny-by-default for an extra-EU PII-bearing replication, admits a within-EU CDN acceleration, and
/// flips an extra-EU target once a lawful mechanism is recorded.
#[test]
fn cdc_10_5_control_plane_enforcement_reads_the_outbound_mirror_policy() {
    let policy = TransferGate::new(); // the REAL §5.2 transfer policy — deny extra-EU by default.
    let mut gate = OutboundMirrorGate::new();
    // ACME is EU-resident (fr-par) — the enforcement gate's residency anchor.
    let mut consumer = ControlPlaneEnforcementConsumer::for_tenant_in(Region::new("fr-par"));

    // ── (1) A PII-bearing extra-EU push-mirror → the policy DENIES by default; the enforcement gate
    //    does NOT push (0 unauthorised cross-residency pushes). ──
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

    // ── (2) A within-EU CDN clone (the Storage 11.2 class, within the tenant's region) → the policy
    //    ALLOWS it (within-EU acceleration); the enforcement gate distributes it. ──
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

    // ── (3) Record a lawful transfer mechanism (the `[OPEN — LEGAL]` counsel-ratified entry) on the
    //    EXISTING transfer policy → the SAME extra-EU push-mirror flips to ALLOWED; the gate CONSULTS
    //    the registry, it is not a blanket block. ──
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

    // The GA-11 green artifact: exactly the one by-default extra-EU deny was counted.
    assert_eq!(
        gate.extra_eu_pii_transfers_blocked(),
        1,
        "0 default extra-EU PII transfers slipped through (the one mirror was blocked before recording)"
    );
}

/// **The disguised-acceleration leg over the policy.** An extra-EU "CDN clone" is NOT an acceleration
/// (no extra-EU edge serves PII, §5.3) — the policy denies it, and the enforcement consumer refuses
/// regardless of the kind label. Proves the kind alone does not exempt a config from the gate.
#[test]
fn cdc_10_5_extra_eu_cdn_clone_is_denied_by_the_policy() {
    let policy = TransferGate::new();
    let mut gate = OutboundMirrorGate::new();
    let mut consumer = ControlPlaneEnforcementConsumer::for_tenant_in(Region::new("fr-par"));

    // The kind is CdnClone but the target is extra-EU — a disguised replication.
    let disguised = OutboundConfig {
        kind: OutboundConfigKind::CdnClone,
        target_region: Region::new("ap-tokyo"),
        pii_bearing: true,
    };
    let decision = consumer.enforce(&mut gate, &policy, &disguised);
    assert!(
        matches!(decision, OutboundDecision::Deny { .. }),
        "an extra-EU CDN clone is a disguised replication — the policy denies it (§5.3)"
    );
    assert_eq!(
        consumer.pushes_enforced, 0,
        "the enforcement gate refuses the disguised extra-EU clone"
    );
    assert_eq!(gate.extra_eu_pii_transfers_blocked(), 1);
}

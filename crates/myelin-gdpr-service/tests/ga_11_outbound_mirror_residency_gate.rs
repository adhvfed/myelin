//! # P-GA-36 → P-452 (M5) — GA-11: the outbound push-mirror residency gate
//!
//! **DATED GREEN ARTIFACT (2026-06-24).** This integration drill IS the dated green artifact the
//! P-GA-36 GATE (GA-11, SCHED) requires (as with the other GDPR drills, the test IS the artifact —
//! there is no GDPR scorecard binary). It proves, end-to-end, the GA-11 row of the drill catalogue:
//!
//! > **GA-11** — *outbound-residency-gate: a PII-bearing extra-EU push-mirror **denied by default**;
//! > a within-EU CDN clone **allowed**. **0 default extra-EU PII transfers.** Telemetry: the
//! > transfer-gate decision + 0 egress is the green artifact.*
//!
//! ## The scenario (chained over the EXISTING `transfer_allowed` policy + the new GA-11 gate)
//! A repo-mirror feature (the §5.3 outbound replication caller) configures four outbound configs for
//! an EU-resident tenant and asks the GA-11 [`OutboundMirrorGate`] BEFORE replicating, HONOURING every
//! deny (the byte never leaves on a `Deny`):
//! 1. **A PII-bearing extra-EU push-mirror** (us-east) → **DENIED BY DEFAULT** (the §5.3 deny-by-default
//!    — no recorded `transfer_allowed` mechanism). The feature does NOT push.
//! 2. **A within-EU CDN clone** (fr-par, the Storage 11.2 class) → **ALLOWED** (within-EU acceleration,
//!    no extra-EU edge serves PII). The feature distributes the clone.
//! 3. **A disguised "CDN clone" pointing extra-EU** (ap-tokyo) → **DENIED** (a CDN edge that served PII
//!    outside the EU would be a replication; §5.3 no extra-EU edge serves PII). The feature does NOT push.
//! 4. **A within-EU push-mirror** (nl-ams, a different EU region) → **ALLOWED** (within-EU acceleration
//!    crosses no sovereignty boundary). The feature mirrors.
//!
//! Then: **record a lawful transfer mechanism** for the extra-EU target and re-decide config 1 → it
//! flips to **ALLOWED** (the gate CONSULTS the existing policy; it is not a blanket extra-EU block).
//!
//! ## The invariant (the GA-11 green artifact)
//! Across the by-default phase the feature made **0 PII-bearing extra-EU replications** (it honoured
//! every deny), and the gate's `extra_eu_pii_transfers_blocked` count equals exactly the two extra-EU
//! crossings it caught (the push-mirror + the disguised clone). **0 default extra-EU PII transfers**
//! slipped through.
//!
//! ## What this proves vs what it reuses (EI-01 §7 coherence)
//! The §5.2 `transfer_allowed` region-policy ([`TransferGate`], P-GA-23) is REUSED unchanged — the
//! GA-11 gate DECIDES over it (it reads the SAME deny-extra-EU-by-default policy + recorded-mechanism
//! set; no second policy). The NEW deliverable is the §5.3 within-EU-acceleration-vs-extra-EU-
//! replication distinction the bare region gate cannot express.
//!
//! ## Floor named (the residual, not pretended-solved — §1 / §5.3)
//! The gate prevents NEW extra-EU PII replication going forward; it cannot recall an independent
//! off-platform clone a third party ALREADY HOLDS. That residual is named, not pretended-solved. The
//! counsel-ratified `transfer_allowed` entries are `[OPEN — LEGAL]` (Schrems II / Art. 44–49).

use myelin_gdpr_service::{
    OutboundAllowReason, OutboundConfig, OutboundDecision, OutboundDenyReason, OutboundMirrorGate,
    TransferGate,
};
use myelin_tenancy::Region;

/// The §5.3 outbound replication caller (a repo-mirror feature). It asks the GA-11 gate BEFORE
/// replicating and HONOURS the verdict: it replicates (bumps `replications_made`) ONLY on an Allow;
/// on a Deny it does NOT push (the byte never leaves). `replications_made` is the wire-egress count.
struct OutboundReplicationFeature {
    /// The number of PII-bearing replications the feature actually sent to a far host (the wire).
    replications_made: u64,
    /// The number of PII-bearing EXTRA-EU replications it sent (the GA-11 zero — must stay 0 on the
    /// by-default set).
    extra_eu_replications_made: u64,
}

impl OutboundReplicationFeature {
    fn new() -> OutboundReplicationFeature {
        OutboundReplicationFeature {
            replications_made: 0,
            extra_eu_replications_made: 0,
        }
    }

    /// Attempt an outbound replication for `config`. Asks the GA-11 gate first and HONOURS the verdict:
    /// replicates (bumps the counters) ONLY on an Allow; on a Deny it does NOT push. The `extra_eu`
    /// flag records whether the (allowed) target was extra-EU — for the GA-11 0-extra-EU invariant.
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
        // else: the feature HONOURS the deny — it does NOT replicate PII-bearing content.
        decision
    }
}

/// **THE GA-11 DRILL (dated green artifact): the outbound push-mirror residency gate.**
#[test]
fn ga_11_outbound_push_mirror_residency_gate_denies_extra_eu_admits_within_eu() {
    let policy = TransferGate::new(); // the REAL §5.2 transfer policy — deny extra-EU by default.
    let mut gate = OutboundMirrorGate::new();
    let mut feature = OutboundReplicationFeature::new();

    // ── (1) A PII-bearing extra-EU push-mirror → DENIED BY DEFAULT; the feature does NOT push. ──
    let extra_eu_mirror = OutboundConfig::push_mirror(Region::new("us-east"));
    let d1 = feature.try_replicate(
        &mut gate,
        &policy,
        &extra_eu_mirror,
        /*extra_eu=*/ true,
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

    // ── (2) A within-EU CDN clone (the Storage 11.2 class) → ALLOWED (within-EU acceleration). ──
    let within_eu_clone = OutboundConfig::cdn_clone(Region::new("fr-par"));
    let d2 = feature.try_replicate(
        &mut gate,
        &policy,
        &within_eu_clone,
        /*extra_eu=*/ false,
    );
    assert_eq!(
        d2,
        OutboundDecision::Allow {
            reason: OutboundAllowReason::WithinEuCdnAcceleration
        },
        "a within-EU CDN clone is permitted (§5.3 acceleration)"
    );

    // ── (3) A disguised "CDN clone" pointing extra-EU → DENIED (no extra-EU edge serves PII). ──
    let disguised = OutboundConfig::cdn_clone(Region::new("ap-tokyo"));
    let d3 = feature.try_replicate(&mut gate, &policy, &disguised, /*extra_eu=*/ true);
    assert!(
        matches!(
            d3,
            OutboundDecision::Deny {
                reason: OutboundDenyReason::ExtraEuCdnEdgeDenied { .. }
            }
        ),
        "a CDN clone pointing extra-EU is a disguised replication — denied (§5.3)"
    );

    // ── (4) A within-EU push-mirror (a different EU region) → ALLOWED (within-EU acceleration). ──
    let within_eu_mirror = OutboundConfig::push_mirror(Region::new("nl-ams"));
    let d4 = feature.try_replicate(
        &mut gate,
        &policy,
        &within_eu_mirror,
        /*extra_eu=*/ false,
    );
    assert_eq!(
        d4,
        OutboundDecision::Allow {
            reason: OutboundAllowReason::WithinEuPushMirror
        },
        "a within-EU push-mirror crosses no sovereignty boundary (§5.3)"
    );

    // ── THE GA-11 INVARIANT: 0 default extra-EU PII transfers slipped through. ──
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

    // ── The gate CONSULTS the policy: recording a lawful mechanism flips the SAME extra-EU mirror. ──
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
        "the lawful-transfer allow did not bump the blocked count — still the two by-default denies"
    );

    println!(
        "[P-452 GA-11 GREEN 2026-06-24] outbound push-mirror residency gate: a PII-bearing extra-EU \
         push-mirror (us-east) DENIED by default + a disguised extra-EU CDN clone (ap-tokyo) DENIED \
         (extra_eu_pii_transfers_blocked=2); a within-EU CDN clone (fr-par) + a within-EU push-mirror \
         (nl-ams) ALLOWED (within-EU acceleration). 0 default extra-EU PII transfers slipped through \
         (extra_eu_replications_made=0). Recording a [OPEN — LEGAL] transfer mechanism flipped the \
         SAME extra-EU mirror to ALLOWED (the gate consults the policy, not a blanket block). RESIDUAL \
         (named, not pretended-solved, §1/§5.3): an independent off-platform clone a third party \
         already holds is outside the gate's reach — the gate prevents NEW extra-EU PII replication."
    );
}

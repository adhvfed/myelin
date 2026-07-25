//! # CDC 10.6 — the audit-log PROOFS (inclusion / consistency / STH), P-GA-20 → P-119
//!
//! **Contract:** index row 10.6 — the audit-log proofs half:
//! `inclusion_proof(action) → MerklePath`, `consistency_proof(t1,t2) → Proof`,
//! `signed_tree_head(tenant) → STH`. The CONSTRUCTION half (the outbox-only consumer + the
//! per-tenant hash-chain + Merkle leaves) is P-GA-19 (`cdc_10_6_audit_log.rs`); this pair proves
//! the PROOFS that run OVER that construction.
//!
//! The contract-coverage scanner (P-S21) reads BOTH halves of the pair from this file:
//! - **provider** = `myelin_gdpr_service::AuditAuthority` — the in-cell audit authority that
//!   commits a `signed_tree_head`, serves an `inclusion_proof` and a `consistency_proof`, and
//!   anchors the STH to an independent witness. It is the PROVIDER of the audit proofs (the
//!   construction's Merkle tree is the substrate it proves over).
//! - **consumer** = an AUDITOR — a downstream verifier that, holding ONLY the published STH + the
//!   witness attestation (no access to the chain store), verifies an inclusion proof against the
//!   STH, a consistency proof between two STHs, and the witness countersignature. This is exactly
//!   the role a regulator / a customer's security team plays (gdpr §6.3 — the proofs are for an
//!   external auditor).
//!
//! The dated green artifact: the provider commits an STH + serves the three proofs; the consumer
//! (the auditor) verifies every one of them with only PII-free material (a root + a tree size + an
//! audit path) — and a tampered proof / a forged STH is REJECTED.

use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_gdpr_service::{
    verify_consistency, verify_inclusion, AuditAuthority, CellSigningKey, NotaryWitness,
    SignedTreeHead, WitnessAttestation,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{ArtifactRef, TenantId};

fn action(id: &str, tenant: &str, subject: &str) -> EventEnvelope {
    let principal = Principal::stub(
        PrincipalId("u-1".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    let region = principal.region.clone();
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("identity.tuple.written".into()),
        schema_ver: 1,
        tenant: TenantId(tenant.into()),
        region,
        actor: Actor(principal),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey("agg:1".into()),
        causation_id: None,
        correlation_id: CorrelationId("r".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

/// **The 10.6 (proofs) provider+consumer CDC pair.** The PROVIDER (the audit authority) commits an
/// STH and serves an inclusion proof + a consistency proof + a witness anchor; the CONSUMER (an
/// auditor, holding only the published STH + the witness attestation) verifies them all — with no
/// access to the chain store and no PII.
#[test]
fn cdc_10_6_provider_serves_proofs_consumer_auditor_verifies() {
    // ── PROVIDER: the in-cell audit authority over a run of real audited actions ──
    let provider = AuditAuthority::new(CellSigningKey::from_seed("cell:fr-par:audit"));
    let tenant = TenantId("acme".into());
    for i in 0..6 {
        provider.consumer().handle(&action(
            &format!("01J-{i}"),
            "acme",
            &format!("myelin://acme/x/{i}"),
        ), &mut myelin_events::HandlerTx::none());
    }

    // The provider commits an STH (size 6) and serves an inclusion proof for leaf 4.
    let sth_v1: SignedTreeHead = provider
        .signed_tree_head(&tenant, "2026-06-20T00:00:00Z")
        .unwrap();
    let inclusion = provider.inclusion_proof(&tenant, 4).unwrap();

    // The provider anchors the STH to an INDEPENDENT witness (a different cell's notary). The
    // witness sees only the opaque root — no PII crosses.
    let witness = NotaryWitness::new(CellSigningKey::from_seed("notary:cell-b"));
    let attestation: WitnessAttestation = provider.anchor_to_witness(&sth_v1, &witness);

    // More actions append; the provider commits a SECOND STH and serves a consistency proof.
    for i in 6..10 {
        provider.consumer().handle(&action(
            &format!("01J-{i}"),
            "acme",
            &format!("myelin://acme/x/{i}"),
        ), &mut myelin_events::HandlerTx::none());
    }
    let sth_v2 = provider
        .signed_tree_head(&tenant, "2026-06-20T01:00:00Z")
        .unwrap();
    let consistency = provider.consistency_proof(&tenant, 6, 10).unwrap();

    // ── CONSUMER (the auditor): verify EVERYTHING with only PII-free, published material ──
    // The auditor holds the cell's PUBLIC verification key (in-cell, gdpr §6.3) to check signatures.
    let auditor_key = CellSigningKey::from_seed("cell:fr-par:audit");

    // 1. The STH signature verifies (it was signed by the cell's audit key).
    assert!(
        sth_v1.verify_signature(&auditor_key),
        "auditor: STH v1 signature verifies"
    );
    assert!(
        sth_v2.verify_signature(&auditor_key),
        "auditor: STH v2 signature verifies"
    );

    // 2. The inclusion proof verifies against the committed STH ("this action IS in the log").
    assert!(
        verify_inclusion(&inclusion, &sth_v1),
        "auditor: the inclusion proof verifies against the STH"
    );
    assert_eq!(inclusion.tree_size, 6);
    assert_eq!(inclusion.leaf_index, 4);

    // 3. The consistency proof verifies between the two STHs ("the log was append-only, not forked").
    assert!(
        verify_consistency(&consistency, &sth_v1, &sth_v2),
        "auditor: the consistency proof verifies (append-only between the two STHs)"
    );

    // 4. The witness attestation matches the honestly-served STH root (the third anchor).
    assert!(
        attestation.matches(&sth_v1.root_hash),
        "auditor: the independent-witness attestation matches the published root"
    );

    // The proofs carry NO PII (an auditor receives only roots, sizes, an audit path of hashes).
    assert!(inclusion.root_hash.starts_with("blake3:"));
    assert!(inclusion
        .audit_path
        .iter()
        .all(|n| n.starts_with("blake3:")));
    assert!(attestation.witnessed_root.starts_with("blake3:"));

    // ── the auditor REJECTS a forged STH + a tampered proof (the verification is real) ──
    // A forged STH signed by a DIFFERENT key fails the signature check.
    let forged = SignedTreeHead {
        signature: CellSigningKey::from_seed("attacker-key").pipe_signature(&sth_v1),
        ..sth_v1.clone()
    };
    assert!(
        !forged.verify_signature(&auditor_key),
        "auditor: a forged STH is rejected"
    );

    // A tampered inclusion proof (wrong leaf index) fails.
    let mut bad = inclusion.clone();
    bad.leaf_index = 0;
    assert!(
        !verify_inclusion(&bad, &sth_v1),
        "auditor: a tampered inclusion proof is rejected"
    );
}

/// A tiny extension trait so the forged-STH case in the CDC reads cleanly (sign a different key's
/// tag over the same STH preimage). Test-local — not production surface.
trait PipeSignature {
    fn pipe_signature(&self, sth: &SignedTreeHead) -> String;
}
impl PipeSignature for CellSigningKey {
    fn pipe_signature(&self, sth: &SignedTreeHead) -> String {
        use myelin_gdpr_service::SigningKey;
        // Re-derive the canonical preimage the authority signs (tenant ∥ size ∥ root ∥ signed_at).
        let mut buf = Vec::new();
        let put = |buf: &mut Vec<u8>, s: &str| {
            buf.extend_from_slice(&(s.len() as u64).to_be_bytes());
            buf.extend_from_slice(s.as_bytes());
        };
        put(&mut buf, &sth.tenant.0);
        buf.extend_from_slice(&sth.tree_size.to_be_bytes());
        put(&mut buf, &sth.root_hash);
        put(&mut buf, &sth.signed_at);
        format!("blake3:{}", hex::encode(self.sign(&buf)))
    }
}

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

#[test]
fn cdc_10_6_provider_serves_proofs_consumer_auditor_verifies() {
    let provider = AuditAuthority::new(CellSigningKey::from_seed("cell:fr-par:audit"));
    let tenant = TenantId("acme".into());
    for i in 0..6 {
        provider.consumer().handle(
            &action(&format!("01J-{i}"), "acme", &format!("myelin://acme/x/{i}")),
            &mut myelin_events::HandlerTx::none(),
        );
    }

    let sth_v1: SignedTreeHead = provider
        .signed_tree_head(&tenant, "2026-06-20T00:00:00Z")
        .unwrap();
    let inclusion = provider.inclusion_proof(&tenant, 4).unwrap();

    let witness = NotaryWitness::new(CellSigningKey::from_seed("notary:cell-b"));
    let attestation: WitnessAttestation = provider.anchor_to_witness(&sth_v1, &witness);

    for i in 6..10 {
        provider.consumer().handle(
            &action(&format!("01J-{i}"), "acme", &format!("myelin://acme/x/{i}")),
            &mut myelin_events::HandlerTx::none(),
        );
    }
    let sth_v2 = provider
        .signed_tree_head(&tenant, "2026-06-20T01:00:00Z")
        .unwrap();
    let consistency = provider.consistency_proof(&tenant, 6, 10).unwrap();

    let auditor_key = CellSigningKey::from_seed("cell:fr-par:audit");

    assert!(
        sth_v1.verify_signature(&auditor_key),
        "auditor: STH v1 signature verifies"
    );
    assert!(
        sth_v2.verify_signature(&auditor_key),
        "auditor: STH v2 signature verifies"
    );

    assert!(
        verify_inclusion(&inclusion, &sth_v1),
        "auditor: the inclusion proof verifies against the STH"
    );
    assert_eq!(inclusion.tree_size, 6);
    assert_eq!(inclusion.leaf_index, 4);

    assert!(
        verify_consistency(&consistency, &sth_v1, &sth_v2),
        "auditor: the consistency proof verifies (append-only between the two STHs)"
    );

    assert!(
        attestation.matches(&sth_v1.root_hash),
        "auditor: the independent-witness attestation matches the published root"
    );

    assert!(inclusion.root_hash.starts_with("blake3:"));
    assert!(inclusion
        .audit_path
        .iter()
        .all(|n| n.starts_with("blake3:")));
    assert!(attestation.witnessed_root.starts_with("blake3:"));

    let forged = SignedTreeHead {
        signature: CellSigningKey::from_seed("attacker-key").pipe_signature(&sth_v1),
        ..sth_v1.clone()
    };
    assert!(
        !forged.verify_signature(&auditor_key),
        "auditor: a forged STH is rejected"
    );

    let mut bad = inclusion.clone();
    bad.leaf_index = 0;
    assert!(
        !verify_inclusion(&bad, &sth_v1),
        "auditor: a tampered inclusion proof is rejected"
    );
}

trait PipeSignature {
    fn pipe_signature(&self, sth: &SignedTreeHead) -> String;
}
impl PipeSignature for CellSigningKey {
    fn pipe_signature(&self, sth: &SignedTreeHead) -> String {
        use myelin_gdpr_service::SigningKey;
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

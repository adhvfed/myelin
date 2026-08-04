use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_gdpr_service::{
    audit, audit_proofs::consistency_proof_over, AuditAuthority, CellSigningKey, NotaryWitness,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{ArtifactRef, TenantId};

fn audit_action(id: &str, tenant: &str, subject: &str) -> EventEnvelope {
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
fn ga_d3_a_retroactive_edit_is_detected_three_independent_ways() {
    let auth = AuditAuthority::new(CellSigningKey::from_seed("cell:fr-par:audit"));
    let tenant = TenantId("acme".into());

    const N: usize = 12;
    for i in 0..N {
        let outcome = auth.consumer().handle(&audit_action(
            &format!("01J-{i}"),
            "acme",
            &format!("myelin://acme/x/{i}"),
        ), &mut myelin_events::HandlerTx::none());
        assert_eq!(outcome, myelin_events::HandleOutcome::Done);
    }

    let published = auth
        .signed_tree_head(&tenant, "2026-06-20T00:00:00Z")
        .expect("an STH");
    assert!(
        published.verify_signature(auth.key()),
        "the published STH verifies in-cell"
    );
    let witness = NotaryWitness::new(CellSigningKey::from_seed("notary:cell-b"));
    let attestation = auth.anchor_to_witness(&published, &witness);
    assert_eq!(
        attestation.witnessed_root, published.root_hash,
        "the witness pinned the opaque root"
    );

    assert!(
        auth.consumer().log().verify_chain(&tenant),
        "baseline: the chain verifies intact"
    );
    let honest_consistency = auth.consistency_proof(&tenant, N as u64, N as u64).unwrap();
    use myelin_gdpr_service::verify_consistency;
    assert!(
        verify_consistency(&honest_consistency, &published, &published),
        "baseline: the consistency proof against the published STH holds"
    );
    let honest_leaves: Vec<[u8; 32]> = auth
        .consumer()
        .log()
        .entries_for(&tenant)
        .iter()
        .map(leaf_of)
        .collect();
    let honest_root = format!("blake3:{}", hex::encode(audit_merkle_root(&honest_leaves)));
    assert!(
        attestation.matches(&honest_root),
        "baseline: the witness matches the honest tree"
    );

    let mut entries = auth.consumer().log().entries_for(&tenant);
    entries[5].subject = ArtifactRef("myelin://acme/TAMPERED".into());

    assert!(
        !audit::verify_entries_for_test(&entries),
        "GA-D3 detection 1/3: the hash-chain breaks on the retroactive edit"
    );

    let tampered_leaves: Vec<[u8; 32]> = entries.iter().map(leaf_of).collect();
    let tampered_consistency =
        consistency_proof_over(&tampered_leaves, N as u64, N as u64).unwrap();
    assert!(
        !verify_consistency(&tampered_consistency, &published, &published),
        "GA-D3 detection 2/3: the consistency proof against the published STH fails"
    );

    let tampered_root = format!(
        "blake3:{}",
        hex::encode(audit_merkle_root(&tampered_leaves))
    );
    assert!(
        !attestation.matches(&tampered_root),
        "GA-D3 detection 3/3: the independent witness mismatches the tampered tree"
    );

}

#[test]
fn ga_d3_a_deleted_entry_is_detected() {
    let auth = AuditAuthority::new(CellSigningKey::from_seed("cell:fr-par:audit"));
    let tenant = TenantId("acme".into());
    for i in 0..8 {
        auth.consumer().handle(&audit_action(
            &format!("01J-{i}"),
            "acme",
            &format!("myelin://acme/x/{i}"),
        ), &mut myelin_events::HandlerTx::none());
    }
    let published = auth.signed_tree_head(&tenant, "t").unwrap();

    let mut entries = auth.consumer().log().entries_for(&tenant);
    entries.remove(3);
    assert!(
        !audit::verify_entries_for_test(&entries),
        "a deleted entry breaks the chain (seq gap + link break)"
    );
    assert_eq!(
        published.tree_size, 8,
        "the STH committed the original size"
    );
    assert_eq!(entries.len(), 7, "the tampered store has one fewer entry");
}

fn audit_merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    let mut level = leaves.to_vec();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut i = 0;
        while i < level.len() {
            if i + 1 < level.len() {
                let mut h = blake3::Hasher::new();
                h.update(&[0x01]);
                h.update(&level[i]);
                h.update(&level[i + 1]);
                next.push(h.finalize().into());
                i += 2;
            } else {
                next.push(level[i]);
                i += 1;
            }
        }
        level = next;
    }
    level[0]
}

fn leaf_of(e: &myelin_gdpr_service::AuditEntry) -> [u8; 32] {
    fn put(buf: &mut Vec<u8>, s: &str) {
        buf.extend_from_slice(&(s.len() as u64).to_be_bytes());
        buf.extend_from_slice(s.as_bytes());
    }
    let mut buf = Vec::new();
    put(&mut buf, &e.tenant.0);
    put(&mut buf, &e.region.0);
    buf.extend_from_slice(&e.seq.to_be_bytes());
    put(&mut buf, &e.actor.actor);
    put(&mut buf, &e.actor.actor_kind);
    put(&mut buf, e.actor.on_behalf_of.as_deref().unwrap_or(""));
    put(&mut buf, &e.action);
    put(&mut buf, &e.subject.0);
    put(&mut buf, e.outcome.as_wire());
    put(&mut buf, &e.correlation_id);
    put(&mut buf, e.causation_id.as_deref().unwrap_or(""));
    put(&mut buf, &e.occurred_at);
    blake3::hash(&buf).into()
}

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::transport::{
    AllowAllAuthority, AuthAction, CollabTransport, DocOp, OpId, OpKind, Recovery, SendOutcome,
};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p-opaque".into()),
        PrincipalKind::Human,
        tenant(),
    )
}

fn transport() -> CollabTransport<AllowAllAuthority> {
    CollabTransport::open(tenant(), "doc-design", AllowAllAuthority).expect("opens")
}

fn op(client: &str, lamport: u64) -> DocOp {
    DocOp::cas(
        OpId::new(client, lamport),
        "actor",
        OpKind::Insert,
        format!("cas:{lamport}").into_bytes(),
    )
}

fn provider_send_op(t: &mut CollabTransport<AllowAllAuthority>, op: DocOp) -> SendOutcome {
    let actor = Principal::stub(
        PrincipalId(op.actor.clone()),
        PrincipalKind::Human,
        tenant(),
    );
    t.send_op(&actor, op)
        .expect("the provider is authorized to edit")
}

fn consumer_connect_backfill(
    t: &mut CollabTransport<AllowAllAuthority>,
    last_seq: u64,
) -> Vec<u64> {
    match t
        .recover(&principal(), AuthAction::Edit, Some(last_seq))
        .expect("an authorized in-window connect resumes")
    {
        Recovery::Resumed { backfill, .. } | Recovery::RebuiltFromLog { backfill, .. } => {
            backfill.iter().map(|p| p.op_seq).collect()
        }
        Recovery::ResyncFromSnapshot { tail, .. } => tail.iter().map(|p| p.op_seq).collect(),
    }
}

#[test]
fn cdc_3_5_knowledge_provider_sends_consumer_reconnects_loses_zero_ops() {
    let mut t = transport();

    for (i, lamport) in [1, 2, 3].iter().enumerate() {
        let out = provider_send_op(&mut t, op("c1", *lamport));
        assert!(out.applied(), "a fresh op applies");
        assert_eq!(
            out.persisted().op_seq,
            (i + 1) as u64,
            "op_seq is monotone (== firehose seq)"
        );
    }

    provider_send_op(&mut t, op("c1", 4));
    provider_send_op(&mut t, op("c1", 5));

    let backfilled = consumer_connect_backfill(&mut t, 2);
    assert_eq!(
        backfilled,
        vec![3, 4, 5],
        "the gap (last_seq, now] is replayed - 0 ops lost"
    );
}

#[test]
fn cdc_3_5_knowledge_redelivered_op_is_idempotent_no_op() {
    let mut t = transport();
    let first = provider_send_op(&mut t, op("c1", 9));
    assert!(first.applied());
    let seq = first.persisted().op_seq;

    let again = provider_send_op(&mut t, op("c1", 9));
    assert!(!again.applied(), "a re-delivered op did NOT freshly apply");
    assert!(
        matches!(again, SendOutcome::Duplicate(_)),
        "it is an idempotent Duplicate no-op"
    );
    assert_eq!(
        again.persisted().op_seq,
        seq,
        "the duplicate resolves to the FIRST op_seq"
    );
    assert_eq!(
        t.head_seq(),
        seq,
        "the head did NOT advance (0 duplicate effect)"
    );
    assert_eq!(
        t.op_count(),
        1,
        "exactly one op persisted (the duplicate was absorbed)"
    );
}

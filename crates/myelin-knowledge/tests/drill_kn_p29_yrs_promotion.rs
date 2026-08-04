use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::block_tree::BlockId;
use myelin_knowledge::transport::{
    AllowAllAuthority, AuthAction, CollabTransport, Connected, DocOp, OpId, OpKind, SendOutcome,
};
use myelin_knowledge::yrs_engine::{DocSnapshot, EnginePromotion, YrsDoc};
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
    CollabTransport::open_with_authority(tenant(), "doc-allhands", AllowAllAuthority)
        .expect("opens")
}

fn bid(s: &str) -> BlockId {
    BlockId(s.to_string())
}

fn cas_op(client: &str, lamport: u64) -> DocOp {
    DocOp::cas(
        OpId::new(client, lamport),
        format!("actor-{client}"),
        OpKind::Insert,
        format!("cas:{client}#{lamport}").into_bytes(),
    )
}

fn yrs_op(client: &str, lamport: u64, update_bytes: Vec<u8>) -> DocOp {
    DocOp::cas(
        OpId::new(client, lamport),
        format!("actor-{client}"),
        OpKind::Insert,
        update_bytes,
    )
}

#[test]
fn kn_d1_re_greens_across_a_real_engine_promote_cutover_zero_lost_zero_dup() {
    let mut t = transport();

    let mut applied: Vec<(u64, OpKind)> = Vec::new();
    for (client, lamport) in [("c1", 1u64), ("c2", 1), ("c1", 2)] {
        let out = t.send_op(cas_op(client, lamport));
        assert!(out.applied(), "each CAS-era op applies");
        let p = out.persisted();
        applied.push((p.op_seq, p.op.kind));
    }
    assert_eq!(t.head_seq(), 3, "three CAS-era ops, op_seq 1..3");

    let mut snapshot = DocSnapshot::new();
    snapshot.push_block(bid("b1"), "intro");
    snapshot.push_block(bid("b2"), "body");
    let promo = EnginePromotion::new(snapshot, t.head_seq());
    assert_eq!(promo.cutover_seq(), 4, "the cutover op_seq is head + 1");
    let cutover_out = t.send_op(promo.cutover_op());
    assert!(
        cutover_out.applied(),
        "the cutover op is an ordinary op on the log"
    );
    assert_eq!(
        cutover_out.persisted().op_seq,
        4,
        "the cutover got the next monotone op_seq (continuity)"
    );
    assert_eq!(cutover_out.persisted().op.kind, OpKind::EnginePromote);
    applied.push((4, OpKind::EnginePromote));

    let live = promo.seeded_doc();
    let u1 = live.edit_block_text(&bid("b1"), 5, "!").unwrap();
    let out = t.send_op(yrs_op("c1", 3, u1));
    assert!(out.applied());
    assert_eq!(out.persisted().op_seq, 5, "Yrs-era op gets op_seq 5");
    applied.push((5, OpKind::Insert));

    let u_inflight = live.edit_block_text(&bid("b2"), 4, "?").unwrap();
    let inflight_op = yrs_op("c2", 2, u_inflight);
    let inflight_first = t.send_op(inflight_op.clone());
    assert!(
        inflight_first.applied(),
        "the in-flight Yrs op reached the server before the sever"
    );
    let inflight_seq = inflight_first.persisted().op_seq;
    applied.push((inflight_seq, OpKind::Insert));
    let c2_cursor = 5u64;

    let u_more = live.edit_block_text(&bid("b1"), 6, " (rev)").unwrap();
    let out = t.send_op(yrs_op("c1", 4, u_more));
    assert!(out.applied());
    assert_eq!(
        out.persisted().op_seq,
        7,
        "while c2 was down, c1 advanced to op_seq 7"
    );
    applied.push((7, OpKind::Insert));

    let connected = t
        .reconnect(&principal(), AuthAction::Edit, c2_cursor)
        .expect("c2 reconnects (warm resume across the cutover)");
    let backfill = match connected {
        Connected::Resumed { backfill } => backfill,
        Connected::ResyncFromSnapshot { tail, .. } => tail,
    };
    let backfill_seqs: Vec<u64> = backfill.iter().map(|p| p.op_seq).collect();
    assert_eq!(
        backfill_seqs,
        vec![6, 7],
        "resume replays (last_seq, now] EXACTLY across the cutover - 0 lost"
    );

    let resend = t.send_op(inflight_op);
    assert!(
        matches!(resend, SendOutcome::Duplicate(_)),
        "the in-flight re-send is an idempotent NO-OP"
    );
    assert_eq!(
        resend.persisted().op_seq,
        inflight_seq,
        "the re-send resolves to its first op_seq (6)"
    );
    assert_eq!(
        t.head_seq(),
        7,
        "the re-send did NOT advance the head (0 duplicate)"
    );

    let seqs: Vec<u64> = applied.iter().map(|(s, _)| *s).collect();
    assert_eq!(
        seqs,
        (1..=7).collect::<Vec<u64>>(),
        "the applied set is op_seq 1..7 contiguous, each op exactly once: 0 lost, 0 duplicate (MEASURED)"
    );

    let reconstructed = YrsDoc::from_state(promo.seed_bytes()).unwrap();
    for p in t
        .reconnect(&principal(), AuthAction::Edit, promo.cutover_seq())
        .map(|c| match c {
            Connected::Resumed { backfill } => backfill,
            Connected::ResyncFromSnapshot { tail, .. } => tail,
        })
        .unwrap()
    {
        if p.op.kind != OpKind::EnginePromote {
            reconstructed.apply_update(&p.op.payload).unwrap();
        }
    }
    assert_eq!(
        reconstructed.block_content(&bid("b1")).unwrap(),
        "intro! (rev)",
        "b1's Yrs-era edits reconstructed across the boundary (0 content lost)"
    );
    assert_eq!(
        reconstructed.block_content(&bid("b2")).unwrap(),
        "body?",
        "b2's in-flight Yrs edit reconstructed across the boundary (0 content lost)"
    );
}

#[test]
fn crdt_convergence_n_clients_same_block_converge_no_blend_lost() {
    const N: usize = 5;
    let mut snapshot = DocSnapshot::new();
    snapshot.push_block(bid("b1"), "");
    let seed = YrsDoc::seed_from_snapshot(&snapshot).encode_state();
    let replicas: Vec<YrsDoc> = (0..N).map(|_| YrsDoc::from_state(&seed).unwrap()).collect();

    let updates: Vec<Vec<u8>> = replicas
        .iter()
        .enumerate()
        .map(|(i, r)| r.edit_block_text(&bid("b1"), 0, &format!("[{i}]")).unwrap())
        .collect();

    for r in &replicas {
        for u in &updates {
            r.apply_update(u).unwrap();
        }
    }

    let states: Vec<String> = replicas
        .iter()
        .map(|r| r.block_content(&bid("b1")).unwrap())
        .collect();
    let first = &states[0];
    for (i, s) in states.iter().enumerate() {
        assert_eq!(
            s, first,
            "replica {i} converged to the same state as replica 0 (no divergence)"
        );
    }
    for i in 0..N {
        assert!(
            first.contains(&format!("[{i}]")),
            "author {i}'s edit survived the merge: {first}"
        );
    }
    assert_eq!(
        first.len(),
        N * 3,
        "exactly N inserts present, no duplication, no loss"
    );
}

#[test]
fn engine_promote_is_reversible_from_the_pre_cutover_snapshot() {
    let mut snapshot = DocSnapshot::new();
    snapshot.push_block(bid("b1"), "original");
    let promo = EnginePromotion::new(snapshot.clone(), 10);
    assert_eq!(
        promo.snapshot(),
        &snapshot,
        "the pre-cutover snapshot is retained for rollback"
    );
    let rolled_back = promo.seeded_doc();
    assert_eq!(rolled_back.block_content(&bid("b1")).unwrap(), "original");
}
